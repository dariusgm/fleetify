//! The agent step end to end: a local bare repository as the remote, a fake host,
//! real git and shell steps, and a fake `claude` script that behaves as the task
//! text asks. Unix only (the scripts run `sh`).
//!
//! The scenarios share one test function on purpose. The fake binary is chosen
//! through an environment variable, and setting that is only sound while no other
//! thread of this test binary runs.
#![cfg(unix)]

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use fleetify::agents::BINARY_ENV;
use fleetify::config::{Config, HostType};
use fleetify::credentials::Credentials;
use fleetify::hosts::{Host, client_for};
use fleetify::recipe::Recipe;
use fleetify::report::RepoResult;
use fleetify::repos::Repo;
use fleetify::run::{Loaded, Mode, RunLayout, WavePlan, Waves, execute_loaded};
use support::{Recorded, serve};

const TOKEN: &str = "tok-agent-test";

/// Stands in for the claude CLI. It runs in the workspace and reads the task file.
/// `BLOCK` in the task leaves blocked.md, `FAIL` exits 1, anything else bumps the
/// python pin. It records its arguments so the test can check them.
const FAKE_CLAUDE: &str = r#"#!/bin/sh
case "$*" in *escape.txt*) echo inside > inside.txt; exit 0 ;; esac
echo "$@" > .agent/args.txt
test -f .agent/task.md || exit 9
test -f .agent/attachments/notes.txt || exit 8
test -f .agent/tools/verify.sh || exit 7
if grep -q BLOCK .agent/task.md; then echo "which python?" > .agent/blocked.md; exit 0; fi
if grep -q FAIL .agent/task.md; then exit 1; fi
sed -i 's/3.11/3.15/' pyproject.toml
echo "changed pyproject" > .agent/result.md
if grep -q COMMIT .agent/task.md; then
  git add -A
  git -c user.name=agent -c user.email=agent@example.invalid commit -q -m "agent commit"
fi
"#;

const VERIFY_OK: &str =
    "#!/bin/sh\ngrep -q '3.15' \"$1/pyproject.toml\" || { echo 'E  not bumped'; exit 1; }\n";

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fleetify-agent-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Seed")
        .env("GIT_AUTHOR_EMAIL", "seed@example.invalid")
        .env("GIT_COMMITTER_NAME", "Seed")
        .env("GIT_COMMITTER_EMAIL", "seed@example.invalid")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A bare remote whose `pyproject.toml` pins python 3.11.
fn remote(base: &Path) -> PathBuf {
    let seed = base.join("seed");
    fs::create_dir_all(&seed).unwrap();
    git(&seed, &["init", "--quiet", "-b", "main"]);
    fs::write(seed.join("pyproject.toml"), "python = \"3.11.*\"\n").unwrap();
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "--quiet", "-m", "initial"]);
    let bare = base.join("remote.git");
    git(
        base,
        &[
            "clone",
            "--quiet",
            "--bare",
            seed.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    bare
}

/// A recipe with one agent instruction, an attachment and a verifier.
fn recipe_dir(base: &Path, instruction: &str) -> PathBuf {
    let dir = base.join("recipe");
    fs::create_dir_all(dir.join("tools")).unwrap();
    fs::write(dir.join("tools/verify.sh"), VERIFY_OK).unwrap();
    fs::write(dir.join("notes.txt"), "use 3.15\n").unwrap();
    fs::write(
        dir.join("recipe.toml"),
        format!(
            r#"schema = 1
name = "bump-python"
branch = "chore/bump-python"
pr_title = "chore: bump python to 3.15"
tools = "tools"
agent = "claude-code"
attachments = ["notes.txt"]

[[instructions]]
text = "{instruction}"

[verify]
argv = ["sh", "{{tools}}/verify.sh", "{{repo}}"]
timeout_s = 60
tail_lines = 10
baseline = false

[guards]
deny_added_patterns = ["NOSONAR"]
"#
        ),
    )
    .unwrap();
    dir
}

/// A fake host: no change request until the first POST; then one is open.
fn host(created: Arc<AtomicBool>) -> support::FakeHost {
    serve(move |req: &Recorded| {
        match (req.method.as_str(), req.target.split('?').next().unwrap()) {
            ("GET", "/repos/o/r") => (200, r#"{"default_branch":"main"}"#.into()),
            ("GET", "/repos/o/r/pulls") if created.load(Ordering::SeqCst) => (
                200,
                r#"[{"number":1,"state":"open","merged_at":null,"html_url":"https://github.com/o/r/pull/1","head":{"sha":"abc"}}]"#
                    .into(),
            ),
            ("GET", "/repos/o/r/pulls") => (200, "[]".into()),
            ("POST", "/repos/o/r/pulls") => {
                created.store(true, Ordering::SeqCst);
                (
                    201,
                    r#"{"number":1,"state":"open","merged_at":null,"html_url":"https://github.com/o/r/pull/1","head":{"sha":"abc"}}"#
                        .into(),
                )
            }
            _ => (404, r#"{"message":"Not Found"}"#.into()),
        }
    })
}

/// Runs the pipeline for one repo with an agent and returns its only result.
fn run_agent_repo(name: &str, instruction: &str) -> (RepoResult, PathBuf, Vec<Recorded>) {
    let base = scratch(name);
    let remote = remote(&base);
    let recipe_dir = recipe_dir(&base, instruction);
    let recipe = Recipe::load(&recipe_dir).unwrap();
    let created = Arc::new(AtomicBool::new(false));
    let host = host(created);
    let workdir = base.join("work");
    let credentials = Credentials::parse(&format!(
        "[[credential]]\nmatch = \"github.com/o/*\"\ntoken = \"{TOKEN}\"\n"
    ))
    .unwrap();
    let repo = Repo {
        host: "github.com".into(),
        host_type: HostType::Github,
        path: "o/r".into(),
        name: "r".into(),
        clone_url: remote.display().to_string(),
        line: 1,
    };
    let client: Box<dyn Host> =
        client_for(HostType::Github, "github.com", Some(&host.base)).unwrap();
    let layout = RunLayout::new(&workdir, name);
    let workspace = layout.workspace_dir(&repo);
    let loaded = Loaded {
        credentials,
        config: Config::default(),
        repos: vec![repo],
        clients: BTreeMap::from([("github.com".to_string(), client)]),
        agent: Some("claude-code".to_string()),
        merge: false,
    };
    let report = execute_loaded(
        &layout,
        &recipe,
        &recipe_dir,
        &workdir,
        loaded,
        Mode::Pipeline { parallel: 1 },
        &Waves {
            plan: &WavePlan::default(),
            proceed: &|_, _| true,
        },
    )
    .unwrap();
    (
        report.repos.into_iter().next().unwrap(),
        workspace,
        host.recorded(),
    )
}

#[test]
fn agent_runs_are_judged_like_any_other_step() {
    let fake = std::env::temp_dir().join(format!("fleetify-fake-claude-{}", std::process::id()));
    fs::write(&fake, FAKE_CLAUDE).unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    // SAFETY: this is the only test in this binary, so no other thread reads the
    // environment while it is set.
    unsafe { std::env::set_var(BINARY_ENV, &fake) };

    // The agent edits the pin, the verifier passes, a pull request opens.
    let (done, workspace, requests) = run_agent_repo("edit", "Bump python to 3.15.");
    assert_eq!(done.result, "pr_open", "{done:?}");
    // The agent's own notes go into the pull request body, marked as unreviewed.
    let body = requests
        .iter()
        .find(|r| r.method == "POST")
        .expect("a pull request was opened");
    assert!(
        body.body.contains("Agent notes (written by the agent"),
        "{}",
        body.body
    );
    assert!(body.body.contains("changed pyproject"), "{}", body.body);
    let args = fs::read_to_string(workspace.join(".agent/args.txt")).unwrap();
    assert!(args.starts_with("-p "), "{args}");
    assert!(
        args.contains("--allowedTools Read Edit Write Glob Grep"),
        "{args}"
    );

    // The agent commits by itself: the orchestrator still makes one commit of the whole change.
    let (committed, _, _) = run_agent_repo("committed", "Bump python to 3.15. COMMIT it yourself.");
    assert_eq!(committed.result, "pr_open", "{committed:?}");

    // The agent asks for a decision: nothing is pushed and the repo needs input.
    let (blocked, _, _) = run_agent_repo("blocked", "Bump python to 3.15. BLOCK if unsure.");
    assert_eq!(blocked.result, "needs_input", "{blocked:?}");
    assert!(!blocked.successful);
    let failure = blocked
        .failure
        .expect("needs_input carries a failure block");
    assert_eq!(failure.output_tail, vec!["which python?".to_string()]);

    // The agent exits non-zero: the repo ends as an error at the agent step.
    let (failed, _, _) = run_agent_repo("failed", "FAIL on purpose.");
    assert_eq!(failed.result, "error", "{failed:?}");
    let message = failed
        .failure
        .expect("error carries a failure block")
        .message;
    assert!(message.starts_with("agent step"), "{message}");

    // The canary passes for a confined agent and leaves its marker behind.
    let checks = scratch("check");
    assert!(!fleetify::agents::is_checked(&checks, "claude-code"));
    fleetify::agents::check(&checks, "claude-code").unwrap();
    assert!(fleetify::agents::is_checked(&checks, "claude-code"));

    let _ = fs::remove_file(&fake);
}
