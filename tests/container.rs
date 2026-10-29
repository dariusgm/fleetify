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

#[test]
#[ignore = "needs docker, the fleetify-agent-opencode image and FLEETIFY_TEST_LLAMACPP_URL"]
fn opencode_in_its_container_answers_from_the_recipe_provider() {
    let Ok(url) = std::env::var("FLEETIFY_TEST_LLAMACPP_URL") else {
        eprintln!("skipped: FLEETIFY_TEST_LLAMACPP_URL is not set");
        return;
    };
    let (root, workspace, _) = fixture("opencode-recipe");
    // The provider key is a placeholder in the recipe. The value reaches the container
    // through `agent_env`, and opencode fills the placeholder from its environment.
    // SAFETY: this test is the only one that sets this variable.
    unsafe { std::env::set_var("FLEETIFY_TEST_KEY", "not-a-real-key") };
    let text = format!(
        r#"schema = 1
name = "opencode-container"
branch = "chore/opencode"
pr_title = "chore: opencode"
agent = "opencode"
image = "fleetify-agent-opencode"
agent_env = ["FLEETIFY_TEST_KEY"]

[[instructions]]
text = "Create a file named inside.txt containing the word inside. Then reply pong."

[agents.opencode.config.provider.llamacpp]
npm = "@ai-sdk/openai-compatible"
name = "llama.cpp"

[agents.opencode.config.provider.llamacpp.options]
baseURL = "{url}"
apiKey = "{{env:FLEETIFY_TEST_KEY}}"

[agents.opencode.config.provider.llamacpp.models."qwen3.8-flash-next"]
name = "Qwen3.8"
tool_call = true

[agents.opencode.config.provider.llamacpp.models."qwen3.8-flash-next".limit]
context = 32768
output = 8192
"#
    );
    let recipe = fleetify::recipe::Recipe::parse(&text).unwrap();
    let invocation = fleetify::agents::invocation_with_prompt(
        "opencode",
        "Create a file named inside.txt containing the word inside. Then reply pong.",
        &recipe,
        &fleetify::grants::Permissions::default(),
        Some("llamacpp/qwen3.8-flash-next"),
        &workspace,
    )
    .unwrap();
    let mut request = RunRequest::new("opencode", invocation.argv);
    request.cwd = Some(workspace.clone());
    request.timeout = Some(Duration::from_secs(900));
    request.env = invocation.env;
    request.on_stop = invocation.stop;
    let result = runner().run(request);
    assert!(
        matches!(result.outcome, Outcome::Exited { code: Some(0) }),
        "{:?}",
        result.outcome
    );
    assert!(
        workspace.join("inside.txt").is_file(),
        "the agent wrote nothing"
    );

    let _ = fs::remove_dir_all(&root);
}
