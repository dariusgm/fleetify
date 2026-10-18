//! Cache clones and workspaces against a real local git repository.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use fleetify::events::{EventBus, Redactor};
use fleetify::git::GitContext;
use fleetify::runner::Runner;
use fleetify::workspace::{create_workspace, ensure_cache, remove_workspaces};

/// A fresh scratch directory for one test.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fleetify-ws-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Runs git in test setup. Panics on failure.
fn git_setup(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A bare "remote" with one commit on `main`. Returns its path.
fn remote_repo(base: &Path) -> PathBuf {
    let work = base.join("seed");
    fs::create_dir_all(&work).unwrap();
    git_setup(&work, &["init", "--quiet", "-b", "main"]);
    fs::write(work.join("README.md"), "hello\n").unwrap();
    git_setup(&work, &["add", "."]);
    git_setup(&work, &["commit", "--quiet", "-m", "initial"]);
    let remote = base.join("remote.git");
    git_setup(
        base,
        &[
            "clone",
            "--quiet",
            "--bare",
            work.to_str().unwrap(),
            remote.to_str().unwrap(),
        ],
    );
    remote
}

fn ctx<'a>(runner: &'a Runner, scratch: &'a Path) -> GitContext<'a> {
    GitContext {
        runner,
        label: "test/repo",
        scratch,
        auth: None,
        timeout: Duration::from_secs(60),
    }
}

fn runner() -> Runner {
    Runner::new(EventBus::new(), Redactor::default())
}

#[test]
fn cache_is_cloned_then_fetched_and_tracks_the_default_branch() {
    let base = scratch("cache");
    let remote = remote_repo(&base);
    let url = remote.to_str().unwrap();
    let cache = base.join("clones/host/group/name");
    let runner = runner();
    let scratch_dir = base.join("tmp");
    let ctx = ctx(&runner, &scratch_dir);

    let first = ensure_cache(&ctx, url, &cache).unwrap();
    assert_eq!(first.default_branch, "main");
    assert_eq!(first.base_sha, git_setup(&remote, &["rev-parse", "main"]));
    assert!(cache.join(".git").exists());

    // A new commit upstream: the second call must fetch and move the cache forward.
    let seed = base.join("seed");
    fs::write(seed.join("NEW.txt"), "new\n").unwrap();
    git_setup(&seed, &["add", "."]);
    git_setup(&seed, &["commit", "--quiet", "-m", "second"]);
    git_setup(&seed, &["push", "--quiet", url, "main"]);

    let second = ensure_cache(&ctx, url, &cache).unwrap();
    assert_ne!(second.base_sha, first.base_sha);
    assert_eq!(second.base_sha, git_setup(&remote, &["rev-parse", "main"]));
    assert_eq!(git_setup(&cache, &["rev-parse", "HEAD"]), second.base_sha);
    let _ = fs::remove_dir_all(&base);
}

#[test]
fn workspace_has_a_branch_at_base_and_a_disabled_push_url() {
    let base = scratch("workspace");
    let remote = remote_repo(&base);
    let url = remote.to_str().unwrap();
    let cache = base.join("cache");
    let runner = runner();
    let scratch_dir = base.join("tmp");
    let ctx = ctx(&runner, &scratch_dir);
    let state = ensure_cache(&ctx, url, &cache).unwrap();

    let workspace = base.join("runs/r1/host/group/name/workspace");
    create_workspace(&ctx, &cache, url, &workspace, &state.base_sha, "chore/test").unwrap();

    assert_eq!(
        git_setup(&workspace, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "chore/test"
    );
    assert_eq!(
        git_setup(&workspace, &["rev-parse", "HEAD"]),
        state.base_sha
    );
    assert_eq!(git_setup(&workspace, &["remote", "get-url", "origin"]), url);
    assert_eq!(
        git_setup(&workspace, &["config", "--get", "remote.origin.pushurl"]),
        fleetify::workspace::DISABLED_PUSH_URL
    );
    let _ = fs::remove_dir_all(&base);
}

#[test]
fn workspace_status_diff_work_and_push_from_inside_fails() {
    let base = scratch("push");
    let remote = remote_repo(&base);
    let url = remote.to_str().unwrap();
    let cache = base.join("cache");
    let runner = runner();
    let scratch_dir = base.join("tmp");
    let ctx = ctx(&runner, &scratch_dir);
    let state = ensure_cache(&ctx, url, &cache).unwrap();
    let workspace = base.join("ws");
    create_workspace(&ctx, &cache, url, &workspace, &state.base_sha, "chore/test").unwrap();

    fs::write(workspace.join("README.md"), "changed\n").unwrap();
    let diff = git_setup(&workspace, &["diff", "--stat"]);
    assert!(diff.contains("README.md"), "{diff}");

    // .agent/ is excluded from status, so reference files never show up as changes.
    fs::create_dir_all(workspace.join(".agent")).unwrap();
    fs::write(workspace.join(".agent/task.md"), "task\n").unwrap();
    let status = git_setup(&workspace, &["status", "--porcelain"]);
    assert!(status.contains("README.md"), "{status}");
    assert!(!status.contains(".agent"), "{status}");

    let push = Command::new("git")
        .args(["push", "origin", "HEAD"])
        .current_dir(&workspace)
        .output()
        .unwrap();
    assert!(!push.status.success(), "push from a workspace must fail");
    let branch_on_remote = Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", "refs/heads/chore/test"])
        .current_dir(&remote)
        .output()
        .unwrap();
    assert!(!branch_on_remote.status.success(), "nothing may be pushed");
    let _ = fs::remove_dir_all(&base);
}

#[test]
fn clean_removes_workspaces_and_keeps_clones() {
    let base = scratch("clean");
    let remote = remote_repo(&base);
    let url = remote.to_str().unwrap();
    let cache = base.join("clones/host/name");
    let runner = runner();
    let scratch_dir = base.join("tmp");
    let ctx = ctx(&runner, &scratch_dir);
    let state = ensure_cache(&ctx, url, &cache).unwrap();

    let run_dir = base.join("runs/r1");
    let workspace = run_dir.join("host/name/workspace");
    create_workspace(&ctx, &cache, url, &workspace, &state.base_sha, "chore/test").unwrap();
    fs::write(run_dir.join("host/name/events.log"), "keep\n").unwrap();

    assert_eq!(remove_workspaces(&run_dir).unwrap(), 1);
    assert!(!workspace.exists());
    assert!(
        run_dir.join("host/name/events.log").exists(),
        "logs are kept"
    );
    assert!(cache.join(".git").exists(), "cache clone is kept");
    let _ = fs::remove_dir_all(&base);
}
