//! The CI-fix loop end to end: a local bare repository as the remote, a fake GitHub host
//! with CI (check runs and Actions logs), a fake `claude` that fixes the failure when it is
//! asked to, and real git steps. Unix only (the fake agent is a shell script).
#![cfg(unix)]

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use fleetify::agents::BINARY_ENV;
use fleetify::config::{Config, HostType};
use fleetify::credentials::Credentials;
use fleetify::hosts::{Host, client_for};
use fleetify::recipe::Recipe;
use fleetify::repos::Repo;
use fleetify::run::{Loaded, Mode, RunLayout, WavePlan, Waves, execute_loaded};
use support::{Recorded, serve};

const TOKEN: &str = "tok-ci-fix-test";
const BRANCH: &str = "fleetify/chore/bump-python";

/// The fake agent. A run whose prompt names the CI failure fixes the failure; the first run
/// bumps the pin. The fix refuses to run unless the failure file has the log line.
const FAKE_CLAUDE: &str = r#"#!/bin/sh
case "$*" in
  *ci-failure.md*)
    grep -q 'ERROR: expected 3.15' .agent/ci-failure.md || exit 5
    echo '# fixed after CI' >> pyproject.toml
    exit 0 ;;
esac
sed -i 's/3.11/3.15/' pyproject.toml
echo "changed pyproject" > .agent/result.md
"#;

const PREPARE_OK: &str = "#!/bin/sh\nexit 0\n";
const VERIFY_OK: &str =
    "#!/bin/sh\ngrep -q '3.15' \"$1/pyproject.toml\" || { echo 'E  not bumped'; exit 1; }\n";

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fleetify-cifix-{name}-{}", std::process::id()));
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
    String::from_utf8_lossy(&output.stdout).into_owned()
}

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

/// A recipe with an agent and a CI wait; `fix_rounds` is written into `[ci]`.
fn recipe_dir(base: &Path, fix_rounds: u32) -> PathBuf {
    let dir = base.join("recipe");
    fs::create_dir_all(dir.join("tools")).unwrap();
    fs::write(dir.join("tools/prepare.sh"), PREPARE_OK).unwrap();
    fs::write(dir.join("tools/verify.sh"), VERIFY_OK).unwrap();
    fs::write(
        dir.join("recipe.toml"),
        format!(
            r#"schema = 1
name = "bump-python"
branch = "chore/bump-python"
pr_title = "chore: bump python to 3.15"
agent = "claude-code"
tools = "tools"

[[instructions]]
text = "Bump python to 3.15."

[verify]
argv = ["sh", "{{tools}}/verify.sh", "{{repo}}"]
timeout_s = 60
tail_lines = 10
baseline = false

[ci]
wait = true
timeout_min = 5
fix_rounds = {fix_rounds}

[limits]
agent_timeout_min = 5
"#
        ),
    )
    .unwrap();
    dir
}

/// Which commits fail: all of them, or only the first one that was asked for.
#[derive(Clone, Copy)]
enum Ci {
    FailFirst,
    AlwaysFail,
}

/// The fake GitHub host. Check runs, Actions runs, jobs and logs all agree on one verdict per
/// commit, and the failing job's log names the version that was expected.
fn host(created: Arc<AtomicBool>, ci: Ci) -> support::FakeHost {
    let first: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    serve(move |req: &Recorded| {
        let path = req.target.split('?').next().unwrap().to_string();
        let query = req
            .target
            .split_once('?')
            .map(|(_, q)| q.to_string())
            .unwrap_or_default();
        let sha_from_path = path
            .strip_prefix("/repos/o/r/commits/")
            .map(|rest| rest.split('/').next().unwrap_or_default().to_string());
        let sha_from_query = query
            .split('&')
            .find_map(|pair| pair.strip_prefix("head_sha="))
            .map(str::to_string);
        if let Some(sha) = sha_from_path.clone().or(sha_from_query.clone()) {
            // The first commit asked for is the one the agent's first change made.
            first.lock().unwrap().get_or_insert_with(|| sha.clone());
        }
        let failing = |sha: &str| match ci {
            Ci::AlwaysFail => true,
            Ci::FailFirst => first.lock().unwrap().as_deref() == Some(sha),
        };
        let conclusion = |sha: &str| if failing(sha) { "failure" } else { "success" };
        match (req.method.as_str(), path.as_str()) {
            ("GET", p) if p.ends_with("/check-runs") => {
                let sha = sha_from_path.unwrap_or_default();
                (
                    200,
                    format!(
                        r#"{{"check_runs":[{{"name":"test","head_sha":"{sha}","status":"completed","conclusion":"{}"}}]}}"#,
                        conclusion(&sha)
                    ),
                )
            }
            ("GET", p) if p.ends_with("/status") => {
                let sha = sha_from_path.unwrap_or_default();
                (200, format!(r#"{{"sha":"{sha}","statuses":[]}}"#))
            }
            ("GET", "/repos/o/r/actions/runs") => {
                // One run per commit: id 7 for a failing commit, id 8 for a passing one.
                let sha = sha_from_query.unwrap_or_default();
                let (id, verdict) = if failing(&sha) { (7, "failure") } else { (8, "success") };
                (
                    200,
                    format!(
                        r#"{{"total_count":1,"workflow_runs":[{{"id":{id},"name":"CI","conclusion":"{verdict}"}}]}}"#
                    ),
                )
            }
            ("GET", "/repos/o/r/actions/runs/7/jobs") => (
                200,
                r#"{"total_count":1,"jobs":[{"id":70,"name":"test","conclusion":"failure","started_at":"2026-10-22T10:00:00Z"}]}"#
                    .into(),
            ),
            ("GET", "/repos/o/r/actions/runs/8/jobs") => (
                200,
                r#"{"total_count":1,"jobs":[{"id":80,"name":"test","conclusion":"success","started_at":"2026-10-22T10:00:00Z"}]}"#
                    .into(),
            ),
            ("GET", "/repos/o/r/actions/jobs/70/logs") => {
                (200, "Run pytest\nE   ERROR: expected 3.15, found 3.11\n".to_string())
            }
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

/// One run of the recipe on the fake host. Returns the result, the workspace and the remote.
fn run(name: &str, fix_rounds: u32, ci: Ci) -> (fleetify::report::RepoResult, PathBuf, PathBuf) {
    let base = scratch(name);
    let remote = remote(&base);
    let recipe_dir = recipe_dir(&base, fix_rounds);
    let recipe = Recipe::load(&recipe_dir).unwrap();
    let created = Arc::new(AtomicBool::new(false));
    let host = host(created, ci);
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
        model: None,
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
    (report.repos.into_iter().next().unwrap(), workspace, remote)
}

fn commits_on_branch(remote: &Path) -> usize {
    git(remote, &["rev-list", "--count", BRANCH])
        .trim()
        .parse()
        .unwrap()
}

#[test]
fn ci_failure_goes_to_the_agent_and_the_fix_is_pushed_until_ci_passes_or_the_rounds_run_out() {
    let fake =
        std::env::temp_dir().join(format!("fleetify-fake-claude-cifix-{}", std::process::id()));
    fs::write(&fake, FAKE_CLAUDE).unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    // SAFETY: this is the only test in this binary, so no other thread reads the
    // environment while it is set.
    unsafe { std::env::set_var(BINARY_ENV, &fake) };

    // One fix round: the first commit fails, the agent fixes it, the fix passes.
    let (fixed, workspace, remote) = run("fix-once", 1, Ci::FailFirst);
    assert_eq!(fixed.result, "ci_passed", "{fixed:?}");
    assert_eq!(commits_on_branch(&remote), 3, "initial, change and fix");
    let failure = fs::read_to_string(workspace.join(".agent/ci-failure.md")).unwrap();
    assert!(failure.contains("CI / test"), "{failure}");
    assert!(failure.contains("ERROR: expected 3.15"), "{failure}");

    // The rounds run out: the fix is pushed once, CI still fails, the repo ends as ci_failed.
    let (stuck, _, remote) = run("fix-exhausted", 1, Ci::AlwaysFail);
    assert_eq!(stuck.result, "ci_failed", "{stuck:?}");
    assert_eq!(commits_on_branch(&remote), 3);

    // No rounds: the failure is reported as it is, nothing is fixed.
    let (plain, workspace, remote) = run("no-rounds", 0, Ci::AlwaysFail);
    assert_eq!(plain.result, "ci_failed", "{plain:?}");
    assert_eq!(commits_on_branch(&remote), 2, "initial and change only");
    assert!(!workspace.join(".agent/ci-failure.md").exists());

    let _ = fs::remove_file(&fake);
}
