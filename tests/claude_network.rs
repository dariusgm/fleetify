//! Network isolation of a Claude Code agent container, against the real docker daemon and the
//! real model endpoint. Ignored by default: it makes model calls with the login on this
//! machine (`~/.claude/.credentials.json`). Needs docker and `fleetify-agent-claude-code`:
//!
//!     cargo test --test claude_network -- --ignored --nocapture
//!
//! Only `api.anthropic.com:443` is listed. Everything else must be refused by the proxy.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use fleetify::agents::invocation_in_network;
use fleetify::container::{self, Run};
use fleetify::egress::{parse_entries, parse_entry};
use fleetify::events::{EventBus, Outcome, Redactor};
use fleetify::grants::Permissions;
use fleetify::isolation::{self, Network};
use fleetify::recipe::Recipe;
use fleetify::runner::{RunRequest, Runner};

const IMAGE: &str = "fleetify-agent-claude-code";
const MODEL_HOST: &str = "api.anthropic.com";

fn runner() -> Runner {
    Runner::new(EventBus::new(), Redactor::default())
}

/// A scratch workspace that is a git repository with one commit, as a real run's is.
fn workspace(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("claude-network-{name}"));
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

/// Runs `script` with node in the Claude Code image, on `network`, and returns its output.
fn node_on(network: &Network, script: &str, workspace: &Path) -> String {
    let name = container::container_name();
    let args = ["-e".to_string(), script.to_string()];
    let names: Vec<String> = network.env.iter().map(|(key, _)| key.clone()).collect();
    let argv = container::run_argv(&Run {
        image: IMAGE,
        name: &name,
        program: "node",
        args: &args,
        workspace,
        directories: &[],
        forwarded: &names,
        files: &[],
        tmpfs: &[],
        network: Some(&network.name),
    })
    .unwrap();
    let mut request = RunRequest::new("node", argv);
    request.timeout = Some(Duration::from_secs(120));
    request.env = container::client_env(names, network.env.clone());
    request.on_stop = Some(container::stop_argv(&name));
    let result = runner().run(request);
    result
        .lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The agent's view of the network, as node sees it. A direct socket bypasses the proxy and
/// must fail. `fetch` honours the proxy (`NODE_USE_ENV_PROXY` is set by the network).
const PROBE: &str = r#"
const net = require('net');
const direct = new Promise((done) => {
  const s = net.connect(443, 'api.anthropic.com', () => { s.destroy(); done('connected'); });
  s.on('error', (e) => done('error ' + e.code));
  s.setTimeout(5000, () => { s.destroy(); done('timeout'); });
});
const probe = (url) => fetch(url).then(
  (r) => 'status ' + r.status,
  (e) => 'error ' + ((e.cause && e.cause.code) || e.message));
(async () => {
  console.log('direct ' + await direct);
  console.log('listed ' + await probe('https://api.anthropic.com/'));
  console.log('unlisted ' + await probe('https://example.com/'));
})();
"#;

#[test]
#[ignore = "needs docker and fleetify-agent-claude-code"]
fn the_claude_container_reaches_the_model_host_and_no_other() {
    let allow = [parse_entry(&format!("{MODEL_HOST}:443")).unwrap()];
    let backend = isolation::backend(isolation::DOCKER).unwrap();
    let base = Path::new(env!("CARGO_TARGET_TMPDIR")).join("claude-network-probe");
    let network = backend.set_up_network(&allow, &base).unwrap();
    let dir = workspace("probe");
    let output = node_on(&network, PROBE, &dir);
    let refused = backend.tear_down_network(&network);

    assert!(
        output.contains("direct error") || output.contains("direct timeout"),
        "a direct connection must bypass nothing:\n{output}"
    );
    assert!(
        output.contains("listed status"),
        "the model host answers through the proxy:\n{output}"
    );
    assert!(
        output.contains("unlisted error"),
        "another host is refused:\n{output}"
    );
    assert!(
        refused.iter().any(|line| line.contains("example.com")),
        "the refused request is reported: {refused:?}"
    );
    assert!(
        !refused.iter().any(|line| line.contains(MODEL_HOST)),
        "the model host is not reported as refused: {refused:?}"
    );
}

#[test]
#[ignore = "needs docker, fleetify-agent-claude-code and the login in ~/.claude"]
fn claude_code_answers_through_the_proxy() {
    let text = format!(
        r#"schema = 1
name = "claude-network"
branch = "chore/claude-network"
pr_title = "chore: claude network"
agent = "claude-code"
image = "{IMAGE}"

[[instructions]]
text = "Reply with the single word pong. Do not use any tool."

[network]
allow = ["{MODEL_HOST}:443"]
"#
    );
    let recipe = Recipe::parse(&text).unwrap();
    let backend = isolation::backend(isolation::DOCKER).unwrap();
    let allow = parse_entries(&recipe.network.allow).unwrap();
    let base = Path::new(env!("CARGO_TARGET_TMPDIR")).join("claude-network-answer");
    let network = backend.set_up_network(&allow, &base).unwrap();
    let dir = workspace("answer");
    let prompt = "Reply with the single word pong. Do not use any tool.";
    let invocation = invocation_in_network(
        "claude-code",
        prompt,
        &recipe,
        &Permissions::default(),
        None,
        &dir,
        Some(&network),
    )
    .unwrap();
    let mut request = RunRequest::new("claude-code", invocation.argv);
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
    // The model host is the only request the agent makes: telemetry and the connector fetch are
    // switched off for a network run (CLAUDE_OFFLINE_ENV).
    assert!(
        refused.is_empty(),
        "the agent made requests outside the model host: {refused:?}"
    );
}
