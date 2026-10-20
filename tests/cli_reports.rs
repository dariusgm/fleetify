//! `report`, `status`, `logs` and refusals, against a run folder written by the library.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use fleetify::events::{Event, Redactor, Stream};
use fleetify::report::{self, RepoResult};
use fleetify::run::{RunLayout, RunMeta};
use fleetify::state::{Recorder, write_atomic};

fn fleetify(workdir: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_fleetify"));
    command.arg("--workdir").arg(workdir);
    command
}

/// A finished run with one successful and one failed repo.
fn fixture(name: &str) -> PathBuf {
    let workdir = std::env::temp_dir().join(format!(
        "fleetify-cli-reports-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&workdir);
    let layout = RunLayout::new(&workdir, "fx");
    let meta = RunMeta {
        run_id: "fx".into(),
        recipe: "recipe".into(),
        started: "2026-10-18T10:00:00Z".into(),
        finished: "2026-10-18T10:01:00Z".into(),
    };
    write_atomic(
        &layout.meta_path(),
        &serde_json::to_vec_pretty(&meta).unwrap(),
    )
    .unwrap();

    let ok = RepoResult::new("github.com/a/good", "dry_run", "would branch");
    let mut bad = RepoResult::new("github.com/a/bad", "error", "clone failed");
    bad.failure = Some(report::Failure {
        class: "error".into(),
        message: "clone failed".into(),
        rule: None,
        output_tail: vec!["fatal: repository not found".into()],
        artifacts: Default::default(),
    });
    report::append_result(&layout.results_path(), &ok).unwrap();
    report::append_result(&layout.results_path(), &bad).unwrap();

    let mut recorder = Recorder::new(layout.clone());
    for repo in ["github.com/a/good", "github.com/a/bad"] {
        recorder.apply(&Event::RepoPhase {
            repo: repo.into(),
            phase: "cloning".into(),
        });
    }
    recorder.apply(&Event::OutputLine {
        command: 1,
        label: "github.com/a/bad".into(),
        stream: Stream::Stderr,
        line: "fatal: repository not found".into(),
    });
    recorder.apply(&Event::RepoFinished {
        repo: "github.com/a/bad".into(),
        result: "error".into(),
        message: "clone failed".into(),
    });
    let _ = Redactor::default();
    workdir
}

#[test]
fn report_failed_lists_only_unsuccessful_repos() {
    let workdir = fixture("failed");
    let output = fleetify(&workdir)
        .args(["report", "--run", "fx", "--failed"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("github.com/a/bad"), "{stdout}");
    assert!(!stdout.contains("github.com/a/good"), "{stdout}");
    let _ = fs::remove_dir_all(&workdir);
}

#[test]
fn report_json_is_valid_and_complete_without_report_file() {
    let workdir = fixture("json");
    let output = fleetify(&workdir)
        .args(["report", "--run", "fx", "--format", "json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["summary"]["total"], 2);
    assert_eq!(value["exit_code"], 1);
    let bad = value["repos"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["repo"] == "github.com/a/bad")
        .unwrap();
    assert_eq!(bad["failure"]["message"], "clone failed");
    assert_eq!(
        bad["failure"]["output_tail"][0],
        "fatal: repository not found"
    );
    let _ = fs::remove_dir_all(&workdir);
}

#[test]
fn status_shows_result_and_last_line_per_repo() {
    let workdir = fixture("status");
    let output = fleetify(&workdir)
        .args(["status", "--run", "fx"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("github.com/a/good"), "{stdout}");
    assert!(stdout.contains("error"), "{stdout}");
    assert!(stdout.contains("fatal: repository not found"), "{stdout}");
    let _ = fs::remove_dir_all(&workdir);
}

#[test]
fn logs_show_the_tail_of_one_repo() {
    let workdir = fixture("logs");
    let output = fleetify(&workdir)
        .args(["logs", "bad", "--run", "fx", "-n", "2"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("fatal: repository not found"), "{stdout}");
    let _ = fs::remove_dir_all(&workdir);
}

#[test]
fn dry_run_with_a_bad_repos_file_is_refused_before_any_work() {
    let workdir = fixture("refused");
    let recipe = workdir.join("recipe");
    fs::create_dir_all(&recipe).unwrap();
    fs::write(
        recipe.join("recipe.toml"),
        "schema = 1\nname = \"x\"\nbranch = \"b\"\npr_title = \"t\"\n",
    )
    .unwrap();
    let repos = workdir.join("repos.txt");
    fs::write(&repos, "owner/name\n").unwrap();
    let output = fleetify(&workdir)
        .args(["run"])
        .arg(&recipe)
        .arg(&repos)
        .args(["--dry-run", "--run", "refused"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(3), "{stderr}");
    assert!(stderr.contains("line 1"), "{stderr}");
    let _ = fs::remove_dir_all(&workdir);
}

fn agent_recipe(workdir: &Path) -> PathBuf {
    let recipe = workdir.join("recipe");
    fs::create_dir_all(&recipe).unwrap();
    fs::write(
        recipe.join("recipe.toml"),
        "schema = 1\nname = \"x\"\nbranch = \"b\"\npr_title = \"t\"\nagent = \"claude-code\"\n\n\
         [[instructions]]\ntext = \"do it\"\n\n[needs]\ncommands = [\"cargo\"]\n",
    )
    .unwrap();
    recipe
}

#[test]
fn a_real_run_needing_an_ungranted_command_is_refused_before_it_starts() {
    let workdir = fixture("ungranted");
    let recipe = agent_recipe(&workdir);
    let repos = workdir.join("repos.txt");
    fs::write(&repos, "github.com/o/r\n").unwrap();
    let output = fleetify(&workdir)
        .args(["run"])
        .arg(&recipe)
        .arg(&repos)
        .args(["--run", "ungranted"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(3), "{stderr}");
    assert!(stderr.contains("command cargo"), "{stderr}");
    assert!(
        !workdir.join("runs/ungranted/github.com").exists(),
        "no repo was touched"
    );
    let _ = fs::remove_dir_all(&workdir);
}

#[test]
fn a_dry_run_reports_the_same_gap_as_a_warning_and_continues() {
    let workdir = fixture("ungranted-dry");
    let recipe = agent_recipe(&workdir);
    let repos = workdir.join("repos.txt");
    fs::write(&repos, "github.com/o/r\n").unwrap();
    let output = fleetify(&workdir)
        .args(["run"])
        .arg(&recipe)
        .arg(&repos)
        .args(["--dry-run", "--run", "ungranted-dry"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("agent claude-code: commands [none]"),
        "{stderr}"
    );
    assert!(
        stderr.contains("dry run: a real run would refuse"),
        "{stderr}"
    );
    assert!(!stderr.contains("refused"), "{stderr}");
    let _ = fs::remove_dir_all(&workdir);
}
