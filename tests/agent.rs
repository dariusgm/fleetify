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
use fleetify::credentials::Credentials;
use fleetify::hosts::HostType;
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
case "$*" in *inside.txt*) echo inside > inside.txt; exit 0 ;; *escape.txt*) exit 0 ;; esac
echo "$@" > .agent/args.txt
test -f .agent/task.md || exit 9
test -f .agent/attachments/notes.txt || exit 8
test -f .agent/tools/verify.sh || exit 7
if grep -q BLOCK .agent/task.md; then echo "which python?" > .agent/blocked.md; exit 0; fi
if grep -q FAIL .agent/task.md; then exit 1; fi
if grep -q LIMIT .agent/task.md; then echo "Error: rate limit reached, try again later" >&2; exit 1; fi
if grep -q CREDITS .agent/task.md; then echo "Fable 5.1 requires usage credits. Switch to another model, or manage usage credits at https://claude.ai/settings/usage"; exit 1; fi
if grep -q WAIT .agent/task.md; then echo "Error: rate limit reached, try again later" >&2; sleep 60; exit 0; fi
if grep -q ASK .agent/task.md; then echo 'Overwrite the lock file? [y/N]'; sleep 30; exit 0; fi
if grep -q FORBIDDEN .agent/task.md; then sed -i 's/3.11/3.15/' pyproject.toml; echo '# NOSONAR' >> pyproject.toml; exit 0; fi
sed -i 's/3.11/3.15/' pyproject.toml
echo "changed pyproject" > .agent/result.md
if grep -q COMMIT .agent/task.md; then
  git add -A
  git -c user.name=agent -c user.email=agent@example.invalid commit -q -m "agent commit"
fi
"#;

/// Stands in for an agent that is not confined: it writes the escape file one level up.
const LEAKY_CLAUDE: &str = r#"#!/bin/sh
case "$*" in *inside.txt*) echo inside > inside.txt; exit 0 ;; *escape.txt*) echo escaped > ../escape.txt; exit 0 ;; esac
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
/// Writes a one-repo recipe. `extra` is appended to the file, for settings a test needs.
fn recipe_dir(base: &Path, instruction: &str, extra: &str) -> PathBuf {
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
{extra}
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
    run_agent_repo_with(name, instruction, "", None)
}

fn run_agent_repo_with(
    name: &str,
    instruction: &str,
    recipe_extra: &str,
    model: Option<&str>,
) -> (RepoResult, PathBuf, Vec<Recorded>) {
    let base = scratch(name);
    let remote = remote(&base);
    let recipe_dir = recipe_dir(&base, instruction, recipe_extra);
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
        repos: vec![repo],
        clients: BTreeMap::from([("github.com".to_string(), client)]),
        agent: Some("claude-code".to_string()),
        model: model.map(str::to_string),
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
        args.contains("--allowedTools Read Glob Grep Edit(//") && !args.contains(" Write"),
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

    // The agent adds a suppression comment: the guard rejects the change, nothing is pushed.
    let (forbidden, _, requests) = run_agent_repo("forbidden", "FORBIDDEN on purpose.");
    assert_eq!(forbidden.result, "guard_violation", "{forbidden:?}");
    assert!(
        !requests.iter().any(|r| r.method == "POST"),
        "no pull request for a rejected change"
    );

    // The agent stops to ask a yes/no question: it is stopped, nothing is pushed, the repo needs input.
    let started = std::time::Instant::now();
    let (asked, _, _) = run_agent_repo("asked", "ASK before changing anything.");
    assert_eq!(asked.result, "needs_input", "{asked:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(25));

    // The agent exits non-zero: the repo ends as an error at the agent step.
    let (failed, _, _) = run_agent_repo("failed", "FAIL on purpose.");
    assert_eq!(failed.result, "error", "{failed:?}");

    // The agent stops on its own rate limit: the repo ends as rate_limited, with the message.
    let (limited, _, _) = run_agent_repo("limited", "LIMIT on purpose.");
    assert_eq!(limited.result, "rate_limited", "{limited:?}");
    assert!(!limited.successful, "{limited:?}");
    let block = limited
        .failure
        .expect("rate_limited carries a failure block");
    assert_eq!(block.class, "rate_limited", "{block:?}");
    assert!(block.message.contains("rate limit reached"), "{block:?}");
    let message = failed
        .failure
        .expect("error carries a failure block")
        .message;
    assert!(message.starts_with("agent step"), "{message}");

    // Missing usage credits (the Fable case): the repo ends as api_limit with the CLI's own
    // sentence, and it is not a rate limit, so a re-run does not wait for it.
    let (credits, _, _) = run_agent_repo("credits", "CREDITS on purpose.");
    assert_eq!(credits.result, "api_limit", "{credits:?}");
    assert!(!credits.successful, "{credits:?}");
    let block = credits.failure.expect("api_limit carries a failure block");
    assert_eq!(block.class, "api_limit", "{block:?}");
    assert!(block.message.contains("usage credits"), "{block:?}");

    // An agent that sits silent after a rate-limit line is stopped at the ceiling
    // (max_wait_min = 0 here), so the run ends as rate_limited and not after 60 seconds.
    let started = std::time::Instant::now();
    let (waited, _, _) = run_agent_repo_with(
        "waited",
        "WAIT on purpose.",
        "\n[rate_limit]\nmax_wait_min = 0\n",
        None,
    );
    assert_eq!(waited.result, "rate_limited", "{waited:?}");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(40),
        "{waited:?}"
    );

    // The model reaches the agent's command line and the report names it for this repo.
    let (modelled, workspace, _) =
        run_agent_repo_with("modelled", "Plain change.", "", Some("fable"));
    assert_eq!(modelled.model.as_deref(), Some("fable"), "{modelled:?}");
    let args = fs::read_to_string(workspace.join(".agent/args.txt")).unwrap();
    assert!(args.contains("--model fable"), "{args}");

    // The canary passes for a confined agent and leaves its marker behind.
    let checks = scratch("check");
    assert!(!fleetify::agents::is_checked(
        &checks,
        "claude-code",
        None,
        None
    ));
    fleetify::agents::check(&checks, "claude-code", None, None).unwrap();
    assert!(fleetify::agents::is_checked(
        &checks,
        "claude-code",
        None,
        None
    ));

    // An agent that writes outside its workspace fails the canary and leaves no marker.
    let leaky = scratch("leaky");
    let leaky_bin = leaky.join("claude");
    fs::write(&leaky_bin, LEAKY_CLAUDE).unwrap();
    fs::set_permissions(&leaky_bin, fs::Permissions::from_mode(0o755)).unwrap();
    // SAFETY: as above, this test is the only one in this binary.
    unsafe { std::env::set_var(BINARY_ENV, &leaky_bin) };
    let leaking = scratch("check-leaky");
    let err = fleetify::agents::check(&leaking, "claude-code", None, None).unwrap_err();
    assert!(err.contains("not confined"), "{err}");
    assert!(!fleetify::agents::is_checked(
        &leaking,
        "claude-code",
        None,
        None
    ));
    unsafe { std::env::set_var(BINARY_ENV, &fake) };

    let _ = fs::remove_file(&fake);
}
