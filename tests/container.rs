//! Agent containers against a real docker daemon and the built agent images. The tests
//! are ignored by default because they need docker and `fleetify-agent-claude-code`
//! (see README, "Containers"). Run them with `cargo test --test container -- --ignored`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use fleetify::container::{self, Run};
use fleetify::events::{EventBus, Outcome, Redactor};
use fleetify::runner::{RunRequest, Runner};

const IMAGE: &str = "fleetify-agent-claude-code";
const SECRET: &str = "decoy-outside-the-workspace";

/// A fresh folder for one test, with a workspace and a decoy file outside it.
fn fixture(name: &str) -> (PathBuf, PathBuf, PathBuf) {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("container-{name}"));
    let _ = fs::remove_dir_all(&root);
    let workspace = root.join("workspace");
    let outside = root.join("outside");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&outside).unwrap();
    let secret = outside.join("secret.txt");
    fs::write(&secret, SECRET).unwrap();
    (root, workspace, secret)
}

fn runner() -> Runner {
    Runner::new(EventBus::new(), Redactor::default())
}

/// The output of a `sh -c` script run in the agent image, confined to `workspace`.
fn in_container(workspace: &Path, script: &str) -> (Outcome, String) {
    let name = container::container_name();
    let args = ["-c".to_string(), script.to_string()];
    let argv = container::run_argv(&Run {
        image: IMAGE,
        name: &name,
        program: "sh",
        args: &args,
        workspace,
        directories: &[],
        forwarded: &[],
        files: &[],
        tmpfs: &[],
    })
    .unwrap();
    let mut request = RunRequest::new("container", argv);
    request.timeout = Some(Duration::from_secs(120));
    request.env = container::client_env(Vec::new(), Vec::new());
    request.on_stop = Some(container::stop_argv(&name));
    let result = runner().run(request);
    let text = result
        .lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    (result.outcome, text)
}

#[test]
#[ignore = "needs docker and the fleetify-agent-claude-code image"]
fn a_host_run_reads_the_decoy_and_the_container_does_not() {
    let (root, workspace, secret) = fixture("decoy");

    // Unconfined: the host run sees the file. This is the behaviour the container must stop.
    let host = Command::new("sh")
        .arg("-c")
        .arg(format!("cat '{}'", secret.display()))
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&host.stdout), SECRET);

    // Confined: the same read in the container finds nothing.
    let (_, text) = in_container(
        &workspace,
        &format!("cat '{}' 2>/dev/null; echo done", secret.display()),
    );
    assert!(
        !text.contains(SECRET),
        "the container read the decoy: {text}"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
#[ignore = "needs docker and the fleetify-agent-claude-code image"]
fn the_container_writes_into_its_workspace_at_the_host_path() {
    let (root, workspace, _) = fixture("write");

    let (outcome, _) = in_container(&workspace, "echo inside > inside.txt");
    assert!(
        matches!(outcome, Outcome::Exited { code: Some(0) }),
        "{outcome:?}"
    );
    assert_eq!(
        fs::read_to_string(workspace.join("inside.txt"))
            .unwrap()
            .trim(),
        "inside"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
#[ignore = "needs docker and the fleetify-agent-claude-code image"]
fn the_agent_cli_runs_in_its_image() {
    let (root, workspace, _) = fixture("cli");

    let (outcome, text) = in_container(&workspace, "claude --version");
    assert!(
        matches!(outcome, Outcome::Exited { code: Some(0) }),
        "{outcome:?}"
    );
    assert!(text.contains("Claude Code"), "{text}");

    let _ = fs::remove_dir_all(&root);
}

#[test]
#[ignore = "needs docker and the fleetify-agent-claude-code image"]
fn a_stopped_run_removes_its_container() {
    let (root, workspace, _) = fixture("stop");
    let name = container::container_name();
    let args = ["-c".to_string(), "sleep 300".to_string()];
    let argv = container::run_argv(&Run {
        image: IMAGE,
        name: &name,
        program: "sh",
        args: &args,
        workspace: &workspace,
        directories: &[],
        forwarded: &[],
        files: &[],
        tmpfs: &[],
    })
    .unwrap();
    let mut request = RunRequest::new("container", argv);
    request.timeout = Some(Duration::from_secs(3));
    request.env = container::client_env(Vec::new(), Vec::new());
    request.on_stop = Some(container::stop_argv(&name));
    let result = runner().run(request);
    assert!(
        matches!(result.outcome, Outcome::TimedOut),
        "{:?}",
        result.outcome
    );

    let listed = Command::new("docker")
        .args(["ps", "-a", "--filter", &format!("name={name}"), "-q"])
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&listed.stdout).trim().is_empty(),
        "the container is still there after the run was stopped"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
#[ignore = "needs docker and the fleetify-agent-claude-code image"]
fn an_image_built_for_another_agent_is_refused() {
    assert!(container::check_image(IMAGE, "claude-code").is_ok());
    let err = container::check_image(IMAGE, "opencode").unwrap_err();
    assert!(err.contains("is for agent claude-code"), "{err}");
}
