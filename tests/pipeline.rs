//! The no-agent pipeline end to end: a local bare repository as the remote, a
//! fake host for the API, and real git and shell steps. Unix only (the test
//! recipes run `sh`).
#![cfg(unix)]

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use fleetify::config::{Config, HostType};
use fleetify::credentials::Credentials;
use fleetify::hosts::{Host, client_for};
use fleetify::recipe::Recipe;
use fleetify::repos::Repo;
use fleetify::run::{Loaded, Mode, RunLayout, WavePlan, Waves, execute_loaded};
use support::serve_with_headers;
use support::{Recorded, serve};

const TOKEN: &str = "tok-pipeline-test";

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fleetify-pipe-{name}-{}", std::process::id()));
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
    fs::write(seed.join("app.py"), "print('hi')\n").unwrap();
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

/// Writes a recipe directory whose prepare and verify steps are the given scripts.
fn recipe_dir(base: &Path, prepare: &str, verify: &str) -> PathBuf {
    let dir = base.join("recipe");
    fs::create_dir_all(dir.join("tools")).unwrap();
    fs::write(dir.join("tools/prepare.sh"), prepare).unwrap();
    fs::write(dir.join("tools/verify.sh"), verify).unwrap();
    fs::write(
        dir.join("recipe.toml"),
        r#"schema = 1
name = "bump-python"
branch = "chore/bump-python"
pr_title = "chore: bump python to 3.15"
tools = "tools"

[prepare]
argv = ["sh", "{tools}/prepare.sh", "{repo}"]
timeout_s = 60

[verify]
argv = ["sh", "{tools}/verify.sh", "{repo}"]
timeout_s = 60
tail_lines = 10
# The verifier checks the bumped state, so it must not run on the base.
baseline = false

[guards]
deny_added_patterns = ["NOSONAR"]
"#,
    )
    .unwrap();
    dir
}

const PREPARE_OK: &str = "#!/bin/sh\nsed -i 's/3.11/3.15/' \"$1/pyproject.toml\"\n";
const VERIFY_OK: &str =
    "#!/bin/sh\ngrep -q '3.15' \"$1/pyproject.toml\" || { echo 'E  not bumped'; exit 1; }\n";

/// A fake host: no change requests yet until the first POST; then one is open.
fn host(created: Arc<AtomicBool>) -> support::FakeHost {
    serve(move |req: &Recorded| base_route(req, &created))
}

/// The fake host with CI: every commit asked for has one completed check run with the
/// given conclusion, and the commit answers for itself.
fn ci_host(created: Arc<AtomicBool>, conclusion: &'static str) -> support::FakeHost {
    serve(move |req: &Recorded| {
        let path = req.target.split('?').next().unwrap();
        if req.method == "GET" && path.starts_with("/repos/o/r/commits/") {
            let sha = path
                .trim_start_matches("/repos/o/r/commits/")
                .trim_end_matches("/check-runs")
                .trim_end_matches("/status");
            if path.ends_with("/check-runs") {
                return (
                    200,
                    format!(
                        r#"{{"check_runs":[{{"name":"test","head_sha":"{sha}","status":"completed","conclusion":"{conclusion}"}}]}}"#
                    ),
                );
            }
            return (200, format!(r#"{{"sha":"{sha}","statuses":[]}}"#));
        }
        base_route(req, &created)
    })
}

fn base_route(req: &Recorded, created: &Arc<AtomicBool>) -> (u16, String) {
    {
        match (req.method.as_str(), req.target.split('?').next().unwrap()) {
        ("GET", "/repos/o/r") => (200, r#"{"default_branch":"main"}"#.into()),
        ("PUT", "/repos/o/r/pulls/1/merge") => (200, r#"{"merged":true}"#.into()),
        ("DELETE", "/repos/o/r/git/refs/heads/fleetify/chore/bump-python") => (204, String::new()),
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
    }
}

struct Scenario {
    base: PathBuf,
    remote: PathBuf,
    recipe_dir: PathBuf,
    host: support::FakeHost,
    created: Arc<AtomicBool>,
}

fn scenario(name: &str, prepare: &str, verify: &str) -> Scenario {
    let base = scratch(name);
    let remote = remote(&base);
    let recipe_dir = recipe_dir(&base, prepare, verify);
    let created = Arc::new(AtomicBool::new(false));
    let host = host(Arc::clone(&created));
    Scenario {
        base,
        remote,
        recipe_dir,
        host,
        created,
    }
}

/// Runs the pipeline for one repo and returns the report's only repo result.
fn run(s: &Scenario, run_id: &str) -> fleetify::report::RepoResult {
    run_with(s, run_id, false, false)
}

/// Like `run`, with the merge policy and the `--merge` flag as given.
fn run_with(
    s: &Scenario,
    run_id: &str,
    merge_without_ci: bool,
    merge: bool,
) -> fleetify::report::RepoResult {
    let recipe = Recipe::load(&s.recipe_dir).unwrap();
    let workdir = s.base.join("work");
    let credentials = Credentials::parse(&format!(
        "[[credential]]\nmatch = \"github.com/o/*\"\ntoken = \"{TOKEN}\"\n"
    ))
    .unwrap();
    let repo = Repo {
        host: "github.com".into(),
        host_type: HostType::Github,
        path: "o/r".into(),
        name: "r".into(),
        clone_url: s.remote.display().to_string(),
        line: 1,
    };
    let client: Box<dyn Host> =
        client_for(HostType::Github, "github.com", Some(&s.host.base)).unwrap();
    let clients = BTreeMap::from([("github.com".to_string(), client)]);
    let layout = RunLayout::new(&workdir, run_id);
    let mut config = Config::default();
    if merge_without_ci {
        config.merge_policy.push(fleetify::config::MergeRule {
            pattern: "github.com/o/*".into(),
            merge_without_ci: true,
            delete_branch: true,
        });
    }
    let loaded = Loaded {
        credentials,
        config,
        repos: vec![repo],
        clients,
        agent: None,
        model: None,
        merge,
    };
    let report = execute_loaded(
        &layout,
        &recipe,
        &s.recipe_dir,
        &workdir,
        loaded,
        Mode::Pipeline { parallel: 2 },
        &Waves {
            plan: &WavePlan::default(),
            proceed: &|_, _| true,
        },
    )
    .unwrap();
    report.repos.into_iter().next().unwrap()
}

fn remote_has_branch(s: &Scenario) -> bool {
    Command::new("git")
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            "refs/heads/fleetify/chore/bump-python",
        ])
        .current_dir(&s.remote)
        .output()
        .unwrap()
        .status
        .success()
}

#[test]
fn happy_path_pushes_one_commit_and_opens_one_pull_request() {
    let s = scenario("happy", PREPARE_OK, VERIFY_OK);
    let result = run(&s, "run-1");
    assert_eq!(result.result, "pr_open", "{result:?}");
    assert!(result.successful);
    assert_eq!(
        result.change_request.as_deref(),
        Some("https://github.com/o/r/pull/1")
    );
    assert!(result.pushed_sha.is_some());
    assert!(remote_has_branch(&s));

    // The pushed branch contains the bump, on top of the base commit.
    let content = git(
        &s.remote,
        &["show", "fleetify/chore/bump-python:pyproject.toml"],
    );
    assert!(content.contains("3.15"), "{content}");

    // The pull request carries the branch, base and title, and the token only in headers.
    let post = s
        .host
        .recorded()
        .into_iter()
        .find(|r| r.method == "POST")
        .unwrap();
    let sent: serde_json::Value = serde_json::from_str(&post.body).unwrap();
    assert_eq!(sent["head"], "fleetify/chore/bump-python");
    assert_eq!(sent["base"], "main");
    assert!(!post.target.contains(TOKEN));
    let _ = fs::remove_dir_all(&s.base);
}

#[test]
fn a_second_run_finds_the_open_pull_request_and_does_nothing() {
    let s = scenario("again", PREPARE_OK, VERIFY_OK);
    assert_eq!(run(&s, "run-1").result, "pr_open");
    let posts_before = s
        .host
        .recorded()
        .iter()
        .filter(|r| r.method == "POST")
        .count();
    let second = run(&s, "run-2");
    assert_eq!(second.result, "already_done", "{second:?}");
    let posts_after = s
        .host
        .recorded()
        .iter()
        .filter(|r| r.method == "POST")
        .count();
    assert_eq!(posts_before, posts_after, "no duplicate pull request");
    let _ = fs::remove_dir_all(&s.base);
}

#[test]
fn push_only_stops_at_the_branch_and_a_second_run_is_already_done() {
    let s = scenario("push-only", PREPARE_OK, VERIFY_OK);
    let recipe = s.recipe_dir.join("recipe.toml");
    let text = fs::read_to_string(&recipe).unwrap() + "\n[delivery]\nopen_mr = false\n";
    fs::write(&recipe, text).unwrap();

    let result = run(&s, "run-1");
    assert_eq!(result.result, "pushed", "{result:?}");
    assert!(result.successful);
    assert!(result.change_request.is_none());
    assert!(remote_has_branch(&s), "the branch must be pushed");
    assert!(
        !s.created.load(Ordering::SeqCst),
        "no pull request in push-only mode"
    );
    assert!(
        !s.host.recorded().iter().any(|r| r.method == "POST"),
        "no change request may be created"
    );

    let again = run(&s, "run-2");
    assert_eq!(again.result, "already_done", "{again:?}");
    let _ = fs::remove_dir_all(&s.base);
}

#[test]
fn a_repo_is_merged_at_the_pushed_commit_when_the_policy_allows_it() {
    let s = scenario("merge", PREPARE_OK, VERIFY_OK);
    let recipe = s.recipe_dir.join("recipe.toml");
    let text = fs::read_to_string(&recipe).unwrap()
        + "\n[ci]\nwait = false\nmerge = \"when_green\"\n\n[delivery]\ndelete_branch = true\n";
    fs::write(&recipe, text).unwrap();

    let result = run_with(&s, "run-1", true, true);
    assert_eq!(result.result, "merged", "{result:?}");
    assert!(result.successful);

    // The merge names the commit that was pushed, so the host refuses if the head moved.
    let requests = s.host.recorded();
    let merge = requests
        .iter()
        .find(|r| r.method == "PUT")
        .expect("a merge request");
    let pushed = result.pushed_sha.clone().expect("a pushed commit");
    assert!(merge.body.contains(&pushed), "{}", merge.body);
    // The source branch is deleted only after the merge succeeded.
    assert!(
        requests.iter().any(|r| r.method == "DELETE"
            && r.target
                .ends_with("/git/refs/heads/fleetify/chore/bump-python")),
        "{requests:?}"
    );
    let _ = fs::remove_dir_all(&s.base);
}

#[test]
fn without_a_merge_rule_nothing_is_merged_and_the_pull_request_stays_open() {
    let s = scenario("no-rule", PREPARE_OK, VERIFY_OK);
    let recipe = s.recipe_dir.join("recipe.toml");
    let text =
        fs::read_to_string(&recipe).unwrap() + "\n[ci]\nwait = false\nmerge = \"when_green\"\n";
    fs::write(&recipe, text).unwrap();

    let result = run_with(&s, "run-1", false, true);
    assert_eq!(result.result, "merge_blocked", "{result:?}");
    assert!(!result.successful);
    assert!(
        !s.host.recorded().iter().any(|r| r.method == "PUT"),
        "no merge request may be sent"
    );
    let _ = fs::remove_dir_all(&s.base);
}

/// A scenario whose recipe waits for CI, with a fake host that answers CI with `conclusion`.
fn ci_scenario(name: &str, conclusion: &'static str) -> Scenario {
    let mut s = scenario(name, PREPARE_OK, VERIFY_OK);
    let created = Arc::new(AtomicBool::new(false));
    s.host = ci_host(Arc::clone(&created), conclusion);
    s.created = created;
    let path = s.recipe_dir.join("recipe.toml");
    let text = fs::read_to_string(&path).unwrap() + "\n[ci]\nwait = true\ntimeout_min = 1\n";
    fs::write(&path, text).unwrap();
    s
}

#[test]
fn ci_passing_on_the_pushed_commit_lets_the_merge_go_ahead() {
    let s = ci_scenario("ci-pass", "success");
    let result = run_with(&s, "ci-pass", true, true);
    assert_eq!(result.result, "merged", "{result:?}");
    assert!(
        s.host
            .recorded()
            .iter()
            .any(|r| r.target.ends_with("/pulls/1/merge")),
        "the merge was requested"
    );
}

#[test]
fn ci_passing_lets_the_merge_go_ahead_without_a_merge_rule() {
    // The merge rule is only for merges without a CI gate. With a passing gate on the
    // pushed commit, `merge = "when_green"` and `--merge` are enough.
    let s = ci_scenario("ci-pass-no-rule", "success");
    let result = run_with(&s, "ci-pass-no-rule", false, true);
    assert_eq!(result.result, "merged", "{result:?}");
}

#[test]
fn failing_ci_on_the_pushed_commit_blocks_the_merge() {
    let s = ci_scenario("ci-fail", "failure");
    let result = run_with(&s, "ci-fail", true, true);
    assert_eq!(result.result, "ci_failed", "{result:?}");
    assert!(!result.successful);
    assert!(
        !s.host
            .recorded()
            .iter()
            .any(|r| r.target.ends_with("/merge")),
        "no merge request after failed CI"
    );
}

#[test]
fn a_failing_verifier_stops_before_any_push() {
    let verify = "#!/bin/sh\necho 'E  pyproject not bumped'\nexit 1\n";
    let s = scenario("verify", PREPARE_OK, verify);
    let result = run(&s, "run-1");
    assert_eq!(result.result, "verify_failed", "{result:?}");
    let failure = result.failure.expect("failure block");
    assert!(
        failure.output_tail.iter().any(|l| l.contains("not bumped")),
        "{failure:?}"
    );
    assert!(!remote_has_branch(&s), "nothing may be pushed");
    assert!(!s.created.load(Ordering::SeqCst), "no pull request");
    let _ = fs::remove_dir_all(&s.base);
}

#[test]
fn a_broken_base_stops_before_prepare_and_any_push() {
    // Baseline on: the verifier fails on the untouched base, so prepare must never run.
    let verify = "#!/bin/sh\necho 'E  base is broken'\nexit 1\n";
    let prepare =
        "#!/bin/sh\ntouch \"$1/prepare-ran\"\nsed -i 's/3.11/3.15/' \"$1/pyproject.toml\"\n";
    let s = scenario("base", prepare, verify);
    let recipe = s.recipe_dir.join("recipe.toml");
    let text = fs::read_to_string(&recipe)
        .unwrap()
        .replace("baseline = false\n", "");
    fs::write(&recipe, text).unwrap();
    let result = run(&s, "run-1");
    assert_eq!(result.result, "base_broken", "{result:?}");
    let failure = result.failure.expect("failure block");
    assert!(
        failure
            .output_tail
            .iter()
            .any(|l| l.contains("base is broken")),
        "{failure:?}"
    );
    let output = fs::read_to_string(&failure.artifacts["output"]).unwrap();
    assert!(
        !output.contains("prepare.sh"),
        "prepare must not run: {output}"
    );
    assert!(!remote_has_branch(&s), "nothing may be pushed");
    assert!(!s.created.load(Ordering::SeqCst), "no pull request");
    let _ = fs::remove_dir_all(&s.base);
}

#[test]
fn an_invalid_ci_file_after_prepare_stops_before_the_build_and_any_push() {
    // The change adds a broken GitHub Actions workflow. The verifier must never run.
    let prepare = "#!/bin/sh\nsed -i 's/3.11/3.15/' \"$1/pyproject.toml\"\nmkdir -p \"$1/.github/workflows\"\nprintf 'jobs: [\\n' > \"$1/.github/workflows/ci.yml\"\n";
    let verify = "#!/bin/sh\ntouch \"$1/verify-ran\"\nexit 1\n";
    let s = scenario("ci", prepare, verify);
    let result = run(&s, "run-1");
    assert_eq!(result.result, "ci_invalid", "{result:?}");
    let failure = result.failure.expect("failure block");
    assert!(
        failure
            .output_tail
            .iter()
            .any(|l| l.contains(".github/workflows/ci.yml")),
        "{failure:?}"
    );
    let output = fs::read_to_string(&failure.artifacts["output"]).unwrap();
    assert!(
        !output.contains("verify.sh"),
        "verifier must not run: {output}"
    );
    assert!(!remote_has_branch(&s), "nothing may be pushed");
    assert!(!s.created.load(Ordering::SeqCst), "no pull request");
    let _ = fs::remove_dir_all(&s.base);
}

#[test]
fn a_guard_violation_stops_before_any_push_and_names_the_rule() {
    let prepare = "#!/bin/sh\nsed -i 's/3.11/3.15/' \"$1/pyproject.toml\"\necho 'x = 1  # NOSONAR' >> \"$1/app.py\"\n";
    let s = scenario("guard", prepare, VERIFY_OK);
    let result = run(&s, "run-1");
    assert_eq!(result.result, "guard_violation", "{result:?}");
    let failure = result.failure.expect("failure block");
    assert_eq!(failure.rule.as_deref(), Some("deny_added_patterns"));
    assert!(
        failure.output_tail.iter().any(|l| l.contains("NOSONAR")),
        "{failure:?}"
    );
    assert!(failure.artifacts.contains_key("diff"));
    assert!(!remote_has_branch(&s));
    let _ = fs::remove_dir_all(&s.base);
}

#[test]
fn a_prepare_step_that_changes_nothing_is_no_changes() {
    let s = scenario("nochange", "#!/bin/sh\nexit 0\n", VERIFY_OK);
    let result = run(&s, "run-1");
    assert_eq!(result.result, "no_changes", "{result:?}");
    assert!(result.successful);
    assert!(!remote_has_branch(&s));
    let _ = fs::remove_dir_all(&s.base);
}

/// A fake host for the change request states of one branch. `open` and `all` answer the
/// two listings the pipeline asks for; `merge` is the answer to the merge call.
fn change_host(
    open: &'static str,
    all: &'static str,
    merge: (u16, &'static str),
) -> support::FakeHost {
    serve(move |req: &Recorded| {
        let path = req.target.split('?').next().unwrap();
        match (req.method.as_str(), path) {
            ("GET", "/repos/o/r") => (200, r#"{"default_branch":"main"}"#.into()),
            ("GET", "/repos/o/r/pulls") if req.target.contains("state=open") => (200, open.into()),
            ("GET", "/repos/o/r/pulls") => (200, all.into()),
            ("PUT", "/repos/o/r/pulls/1/merge") => (merge.0, merge.1.into()),
            _ => (404, r#"{"message":"Not Found"}"#.into()),
        }
    })
}

const MERGED_PR: &str = r#"[{"number":1,"state":"closed","merged_at":"2026-10-21T08:00:00Z","html_url":"https://github.com/o/r/pull/1","head":{"sha":"abc"}}]"#;
const OPEN_PR: &str = r#"[{"number":1,"state":"open","merged_at":null,"html_url":"https://github.com/o/r/pull/1","head":{"sha":"abc"}}]"#;

#[test]
fn a_merge_whose_response_was_lost_is_not_repeated_on_the_next_run() {
    // No open change request, but the branch's change request is merged: the repo is done,
    // and no second change request is opened.
    let mut s = scenario("lost-merge", PREPARE_OK, VERIFY_OK);
    s.host = change_host("[]", MERGED_PR, (500, "{}"));
    let result = run_with(&s, "lost-merge", false, true);
    assert_eq!(result.result, "already_done", "{result:?}");
    assert!(result.detail.contains("already merged"), "{result:?}");
    let requests = s.host.recorded();
    assert!(
        !requests
            .iter()
            .any(|r| r.method == "POST" || r.method == "PUT"),
        "{requests:?}"
    );
    let _ = fs::remove_dir_all(&s.base);
}

#[test]
fn a_change_request_merged_during_the_run_is_reported_as_merged() {
    // The merge call is never sent: the host already shows the change request as merged.
    let mut s = scenario("merged-before-put", PREPARE_OK, VERIFY_OK);
    s.host = change_host(OPEN_PR, MERGED_PR, (500, "{}"));
    let recipe = s.recipe_dir.join("recipe.toml");
    let text =
        fs::read_to_string(&recipe).unwrap() + "\n[ci]\nwait = false\nmerge = \"when_green\"\n";
    fs::write(&recipe, text).unwrap();
    let result = run_with(&s, "merged-before-put", true, true);
    assert_eq!(result.result, "merged", "{result:?}");
    assert!(
        !s.host.recorded().iter().any(|r| r.method == "PUT"),
        "no merge call expected"
    );
    let _ = fs::remove_dir_all(&s.base);
}

#[test]
fn a_host_that_fails_during_the_merge_ends_the_repo_as_host_unreachable() {
    let mut s = scenario("merge-503", PREPARE_OK, VERIFY_OK);
    s.host = change_host(OPEN_PR, OPEN_PR, (503, r#"{"message":"unavailable"}"#));
    let recipe = s.recipe_dir.join("recipe.toml");
    let text =
        fs::read_to_string(&recipe).unwrap() + "\n[ci]\nwait = false\nmerge = \"when_green\"\n";
    fs::write(&recipe, text).unwrap();
    let result = run_with(&s, "merge-503", true, true);
    assert_eq!(result.result, "host_unreachable", "{result:?}");
    assert!(!result.successful);
    let failure = result.failure.expect("a failure block");
    assert_eq!(failure.class, "host_unreachable");
    assert!(!failure.message.is_empty());
    let _ = fs::remove_dir_all(&s.base);
}

#[test]
fn a_rate_limit_longer_than_the_ceiling_ends_the_repo_as_rate_limited() {
    let mut s = scenario("merge-429", PREPARE_OK, VERIFY_OK);
    s.host = serve_with_headers(|req| {
        let path = req.target.split('?').next().unwrap();
        match (req.method.as_str(), path) {
            ("GET", "/repos/o/r") => (200, Vec::new(), r#"{"default_branch":"main"}"#.into()),
            ("GET", "/repos/o/r/pulls") if req.target.contains("state=open") => {
                (200, Vec::new(), OPEN_PR.into())
            }
            ("GET", "/repos/o/r/pulls") => (200, Vec::new(), OPEN_PR.into()),
            ("PUT", "/repos/o/r/pulls/1/merge") => (
                429,
                vec![("Retry-After".into(), "3600".into())],
                "{}".into(),
            ),
            _ => (404, Vec::new(), r#"{"message":"Not Found"}"#.into()),
        }
    });
    let recipe = s.recipe_dir.join("recipe.toml");
    let text =
        fs::read_to_string(&recipe).unwrap() + "\n[ci]\nwait = false\nmerge = \"when_green\"\n";
    fs::write(&recipe, text).unwrap();
    let result = run_with(&s, "merge-429", true, true);
    assert_eq!(result.result, "rate_limited", "{result:?}");
    assert!(!result.successful);
    let failure = result.failure.expect("a failure block");
    assert_eq!(failure.class, "rate_limited");
    assert!(failure.message.contains("3600"), "{}", failure.message);
    let _ = fs::remove_dir_all(&s.base);
}
