//! Network isolation of a Copilot CLI agent container, against the real docker daemon and the
//! real Copilot endpoints. Ignored by default: it makes model calls with the GitHub login.
//! Needs docker, `fleetify-agent-copilot` and a Copilot-only token in `COPILOT_GITHUB_TOKEN`
//! (a fine-grained token with the Copilot Requests permission and nothing else). Do not use the
//! broad `gh` token here:
//!
//!     COPILOT_GITHUB_TOKEN="$(cat ~/.config/fleetify/copilot-test-token)" \
//!         cargo test --test copilot_network -- --ignored --nocapture
//!
//! The token reaches the container by name only (`agent_env`), as a recipe passes it.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use fleetify::agents::invocation_in_network;
use fleetify::egress::parse_entries;
use fleetify::events::{EventBus, Outcome, Redactor};
use fleetify::grants::Permissions;
use fleetify::isolation;
use fleetify::recipe::Recipe;
use fleetify::runner::{RunRequest, Runner};

fn runner() -> Runner {
    Runner::new(EventBus::new(), Redactor::default())
}

/// A scratch workspace that is a git repository with one commit, as a real run's is.
fn workspace(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("copilot-network-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("README.md"), "scratch repository\n").unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["add", "README.md"],
        vec![
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-q",
            "-m",
            "initial",
        ],
    ] {
        let status = Command::new("git")
            .args(&args)
            .current_dir(&dir)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }
    dir
}

#[test]
#[ignore = "needs docker, fleetify-agent-copilot, COPILOT_GITHUB_TOKEN and a Copilot account"]
fn copilot_answers_through_the_proxy() {
    if std::env::var("COPILOT_GITHUB_TOKEN").is_err() {
        eprintln!("skipped: COPILOT_GITHUB_TOKEN is not set");
        return;
    }
    let text = r#"schema = 1
name = "copilot-network"
branch = "chore/copilot-network"
pr_title = "chore: copilot network"
agent = "copilot"
image = "fleetify-agent-copilot"
agent_env = ["COPILOT_GITHUB_TOKEN"]

[[instructions]]
text = "Reply with the single word pong. Do not use any tool."

[network]
allow = ["api.individual.githubcopilot.com:443", "telemetry.individual.githubcopilot.com:443", "api.github.com:443", "github.com:443"]
"#;
    let recipe = Recipe::parse(text).unwrap();
    let backend = isolation::backend(isolation::DOCKER).unwrap();
    let allow = parse_entries(&recipe.network.allow).unwrap();
    let base = Path::new(env!("CARGO_TARGET_TMPDIR")).join("copilot-network");
    let network = backend.set_up_network(&allow, &base).unwrap();
    let dir = workspace("answer");
    let prompt = "Reply with the single word pong. Do not use any tool.";
    let invocation = invocation_in_network(
        "copilot",
        prompt,
        &recipe,
        &Permissions::default(),
        None,
        &dir,
        Some(&network),
    )
    .unwrap();
    let mut request = RunRequest::new("copilot", invocation.argv);
    request.cwd = Some(dir.clone());
    request.timeout = Some(Duration::from_secs(300));
    request.env = invocation.env;
    request.on_stop = invocation.stop;
    let result = runner().run(request);
    let refused = backend.tear_down_network(&network);
    let output: Vec<String> = result.lines.iter().map(|line| line.text.clone()).collect();
    for line in output.iter().rev().take(8).rev() {
        eprintln!("| {line}");
    }
    for denied in &refused {
        eprintln!("refused by the proxy: {denied}");
    }

    assert!(
        matches!(result.outcome, Outcome::Exited { code: Some(0) }),
        "{:?}",
        result.outcome
    );
    assert!(
        output
            .iter()
            .any(|line| line.to_lowercase().contains("pong")),
        "the model answered: {output:?}"
    );
    // Every request is to a listed host: the model, the telemetry endpoint and GitHub.
    assert!(
        refused.is_empty(),
        "a request outside the allow list was refused: {refused:?}"
    );
}
