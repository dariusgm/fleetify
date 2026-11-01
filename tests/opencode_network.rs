//! Network isolation of an opencode agent container, against the real docker daemon and the
//! local llama.cpp server. Ignored by default. Needs docker, the image `fleetify-agent-opencode`
//! and `FLEETIFY_TEST_LLAMACPP_URL`, for example `http://192.168.178.151:9931/v1`:
//!
//!     FLEETIFY_TEST_LLAMACPP_URL=http://192.168.178.151:9931/v1 \
//!         cargo test --test opencode_network -- --ignored --nocapture
//!
//! The port after the model's port is another service on the same host (on this machine, the
//! embedding server). It must be refused, because only the model's port is listed.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use fleetify::agents::invocation_in_network;
use fleetify::container::{self, Run};
use fleetify::egress::parse_entry;
use fleetify::events::{EventBus, Outcome, Redactor};
use fleetify::grants::Permissions;
use fleetify::isolation::{self, Network};
use fleetify::recipe::Recipe;
use fleetify::runner::{RunRequest, Runner};

const IMAGE: &str = "fleetify-agent-opencode";

/// The model's host and port, from `FLEETIFY_TEST_LLAMACPP_URL`. None when it is not set.
fn model_endpoint() -> Option<(String, u16)> {
    let url = std::env::var("FLEETIFY_TEST_LLAMACPP_URL").ok()?;
    let authority = url
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .split('/')
        .next()?
        .to_string();
    let (host, port) = authority.rsplit_once(':')?;
    Some((host.to_string(), port.parse().ok()?))
}

fn runner() -> Runner {
    Runner::new(EventBus::new(), Redactor::default())
}

/// A scratch workspace that is a git repository with one commit, as a real run's is.
fn workspace(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("opencode-network-{name}"));
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

/// Runs `script` with node in the opencode image, on `network`, and returns its output.
fn node_on(network: &Network, script: &str, env: &[(String, String)], workspace: &Path) -> String {
    let name = container::container_name();
    let args = ["-e".to_string(), script.to_string()];
    let mut names: Vec<String> = network.env.iter().map(|(key, _)| key.clone()).collect();
    names.extend(env.iter().map(|(key, _)| key.clone()));
    let mut set = network.env.clone();
    set.extend(env.iter().cloned());
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
    request.env = container::client_env(names, set);
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
/// must fail. `fetch` honours the proxy variables (`NODE_USE_ENV_PROXY` is set by the network).
const PROBE: &str = r#"
const net = require('net');
const host = process.env.MODEL_HOST;
const port = +process.env.MODEL_PORT;
const direct = new Promise((done) => {
  const s = net.connect(port, host, () => { s.destroy(); done('connected'); });
  s.on('error', (e) => done('error ' + e.code));
  s.setTimeout(5000, () => { s.destroy(); done('timeout'); });
});
const probe = (url) => fetch(url).then(
  (r) => 'status ' + r.status,
  (e) => 'error ' + ((e.cause && e.cause.code) || e.message));
(async () => {
  console.log('direct ' + await direct);
  console.log('listed ' + await probe(`http://${host}:${port}/v1/models`));
  console.log('unlisted ' + await probe(`http://${host}:${port + 1}/v1/models`));
})();
"#;

#[test]
#[ignore = "needs docker, fleetify-agent-opencode and FLEETIFY_TEST_LLAMACPP_URL"]
fn the_opencode_container_reaches_the_listed_model_port_and_no_other() {
    let Some((host, port)) = model_endpoint() else {
        eprintln!("skipped: FLEETIFY_TEST_LLAMACPP_URL is not set");
        return;
    };
    let allow = [parse_entry(&format!("{host}:{port}")).unwrap()];
    let backend = isolation::backend(isolation::DOCKER).unwrap();
    let base = Path::new(env!("CARGO_TARGET_TMPDIR")).join("opencode-network-probe");
    let network = backend.set_up_network(&allow, &base).unwrap();
    let dir = workspace("probe");
    let env = [
        ("MODEL_HOST".to_string(), host.clone()),
        ("MODEL_PORT".to_string(), port.to_string()),
    ];
    let output = node_on(&network, PROBE, &env, &dir);
    let refused = backend.tear_down_network(&network);

    assert!(
        output.contains("direct error") || output.contains("direct timeout"),
        "a direct connection must bypass nothing:\n{output}"
    );
    assert!(
        output.contains("listed status 200"),
        "the listed model port answers through the proxy:\n{output}"
    );
    assert!(
        output.contains("unlisted error"),
        "the other port on the same host is refused:\n{output}"
    );
    assert!(
        refused
            .iter()
            .any(|line| line.contains(&format!(":{}", port + 1))),
        "the refused request is reported: {refused:?}"
    );
    assert!(
        !refused
            .iter()
            .any(|line| line.contains(&format!("{host}:{port}"))),
        "the listed request is not reported as refused: {refused:?}"
    );
}

#[test]
#[ignore = "needs docker, fleetify-agent-opencode and FLEETIFY_TEST_LLAMACPP_URL"]
fn opencode_cannot_answer_from_a_model_port_the_recipe_does_not_list() {
    let Some((host, port)) = model_endpoint() else {
        eprintln!("skipped: FLEETIFY_TEST_LLAMACPP_URL is not set");
        return;
    };
    // The provider points at the other port on the model's host; the recipe lists only the
    // model's own port. opencode must not get an answer, so it must not write the file.
    let other = port + 1;
    let text = format!(
        r#"schema = 1
name = "opencode-unlisted"
branch = "chore/opencode"
pr_title = "chore: opencode"
agent = "opencode"
image = "fleetify-agent-opencode"

[[instructions]]
text = "Create a file named inside.txt containing the word inside. Then reply pong."

[agents.opencode.config.provider.llamacpp]
npm = "@ai-sdk/openai-compatible"
name = "llama.cpp"

[agents.opencode.config.provider.llamacpp.options]
baseURL = "http://{host}:{other}/v1"
apiKey = "not-a-real-key"

[agents.opencode.config.provider.llamacpp.models."qwen3.8-flash-next"]
name = "Qwen3.8"
tool_call = true

[network]
allow = ["{host}:{port}"]
"#
    );
    let recipe = Recipe::parse(&text).unwrap();
    let backend = isolation::backend(isolation::DOCKER).unwrap();
    let allow = fleetify::egress::parse_entries(&recipe.network.allow).unwrap();
    let base = Path::new(env!("CARGO_TARGET_TMPDIR")).join("opencode-network-unlisted");
    let network = backend.set_up_network(&allow, &base).unwrap();
    let dir = workspace("unlisted");
    let invocation = invocation_in_network(
        "opencode",
        "Create a file named inside.txt containing the word inside. Then reply pong.",
        &recipe,
        &Permissions::default(),
        Some("llamacpp/qwen3.8-flash-next"),
        &dir,
        Some(&network),
    )
    .unwrap();
    let mut request = RunRequest::new("opencode", invocation.argv);
    request.cwd = Some(dir.clone());
    request.timeout = Some(Duration::from_secs(600));
    request.env = invocation.env;
    request.on_stop = invocation.stop;
    let result = runner().run(request);
    let refused = backend.tear_down_network(&network);

    assert!(
        !dir.join("inside.txt").exists(),
        "the agent wrote a file without an answer from the model: {:?}",
        result.outcome
    );
    assert!(
        refused
            .iter()
            .any(|line| line.contains(&format!(":{other}"))),
        "the refused model request is reported: {refused:?}"
    );
    assert!(
        !matches!(result.outcome, Outcome::Exited { code: Some(0) }),
        "the run must not succeed without the model: {:?}",
        result.outcome
    );
}
