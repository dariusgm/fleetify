//! The network of an agent container, against the real docker daemon (stage 3). Ignored by
//! default: run with `cargo test --test network -- --ignored`. The upstreams are containers on
//! the default bridge, so the test needs no internet access. It needs the image
//! `node:22-bookworm-slim` locally and the proxy image `fleetify-proxy:1` (see docker/proxy).

use std::process::Command;

use fleetify::egress::parse_entry;
use fleetify::isolation::{self, Network};

/// Runs one docker command and returns its trimmed stdout.
fn docker(args: &[&str]) -> String {
    let output = Command::new("docker")
        .args(args)
        .output()
        .expect("docker runs");
    assert!(
        output.status.success(),
        "docker {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A small upstream on the default bridge that answers every request with `ok`.
fn upstream(name: &str) -> String {
    let script = "require('http').createServer((q,s)=>s.end('ok')).listen(8080)";
    docker(&[
        "run",
        "-d",
        "--rm",
        "--pull",
        "never",
        "--name",
        name,
        "node:22-bookworm-slim",
        "node",
        "-e",
        script,
    ]);
    docker(&[
        "inspect",
        "-f",
        "{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}",
        name,
    ])
}

/// Runs `script` in a container on `network` with the run's proxy variables and returns its
/// output. `TARGET` names the URL the script asks for.
fn agent(network: &Network, script: &str, target: &str) -> String {
    let mut args = vec![
        "run".to_string(),
        "--rm".to_string(),
        "--pull".to_string(),
        "never".to_string(),
        "--network".to_string(),
        network.name.clone(),
        "-e".to_string(),
        format!("TARGET={target}"),
    ];
    for (key, value) in &network.env {
        args.push("-e".to_string());
        args.push(format!("{key}={value}"));
    }
    args.extend([
        "node:22-bookworm-slim".to_string(),
        "node".to_string(),
        "-e".to_string(),
        script.to_string(),
    ]);
    let output = Command::new("docker")
        .args(&args)
        .output()
        .expect("docker runs");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Asks the proxy for `TARGET` and prints the status line it answers with.
const VIA_PROXY: &str = "\
const net = require('net');
const [host, port] = process.env.http_proxy.replace('http://', '').split(':');
const s = net.connect(+port, host, () => {
  s.write(`GET ${process.env.TARGET} HTTP/1.1\\r\\nHost: x\\r\\nConnection: close\\r\\n\\r\\n`);
});
s.on('data', (d) => { console.log(String(d).split('\\r\\n')[0]); s.destroy(); });
s.on('error', (e) => console.log('error ' + e.code));
setTimeout(() => { console.log('timeout'); process.exit(0); }, 5000).unref();
";

/// Fetches `TARGET` with Node's fetch, the client the Node-based agents use. It goes through
/// the proxy only when the run sets `NODE_USE_ENV_PROXY`.
const FETCH: &str = "\
fetch(process.env.TARGET)
  .then((r) => console.log('status ' + r.status))
  .catch((e) => console.log('error ' + (e.cause?.code || e.cause?.message || e.message)));
";

/// Connects to `TARGET` directly, without the proxy, and prints the outcome.
const DIRECT: &str = "\
const net = require('net');
const [host, port] = process.env.TARGET.split(':');
const s = net.connect(+port, host, () => { console.log('connected'); s.destroy(); });
s.on('error', (e) => console.log('error ' + e.code));
setTimeout(() => { console.log('timeout'); process.exit(0); }, 5000).unref();
";

#[test]
#[ignore = "needs the docker daemon, node:22-bookworm-slim and fleetify-proxy:1"]
fn the_agent_reaches_only_listed_targets_and_the_refusals_are_reported() {
    let allowed = upstream("fleetify-net-test-allowed");
    let other = upstream("fleetify-net-test-other");
    let allow = [parse_entry(&format!("{allowed}:8080")).unwrap()];
    let backend = isolation::backend(isolation::DOCKER).unwrap();
    let base = std::env::temp_dir().join("fleetify-net-test");
    let network = backend
        .set_up_network(&allow, &base)
        .expect("the network starts");

    let listed = agent(&network, VIA_PROXY, &format!("http://{allowed}:8080/"));
    let unlisted = agent(&network, VIA_PROXY, &format!("http://{other}:8080/"));
    let direct = agent(&network, DIRECT, &format!("{allowed}:8080"));
    let fetched_listed = agent(&network, FETCH, &format!("http://{allowed}:8080/"));
    let fetched_unlisted = agent(&network, FETCH, &format!("http://{other}:8080/"));
    let refused = backend.tear_down_network(&network);

    docker(&[
        "rm",
        "-f",
        "fleetify-net-test-allowed",
        "fleetify-net-test-other",
    ]);
    assert!(
        !network.dir.exists(),
        "the configuration directory is removed"
    );
    assert_eq!(listed, "HTTP/1.1 200 OK", "the listed target answers");
    assert_eq!(
        fetched_listed, "status 200",
        "node's fetch reaches the listed target through the proxy"
    );
    // Node sends plain http:// requests through CONNECT as well, so a refused tunnel shows as
    // a failed request, not as a 403. The refusal is still logged (checked below).
    assert!(
        fetched_unlisted.starts_with("error"),
        "node's fetch is refused for an unlisted target, got {fetched_unlisted:?}"
    );
    assert_eq!(
        unlisted, "HTTP/1.1 403 Forbidden",
        "an unlisted target is refused by the proxy"
    );
    assert!(
        direct.starts_with("error") || direct == "timeout",
        "a direct connection bypassing the proxy must fail, got {direct:?}"
    );
    assert!(
        refused.iter().any(|line| line.contains(&other)),
        "the refused request is reported: {refused:?}"
    );
    assert!(
        !refused.iter().any(|line| line.contains(&allowed)),
        "the listed request is not reported as refused: {refused:?}"
    );
}

#[test]
#[ignore = "needs the docker daemon, node:22-bookworm-slim and fleetify-proxy:1"]
fn two_runs_cannot_reach_each_others_proxy() {
    let target = upstream("fleetify-net-test-peer");
    let allow = [parse_entry(&format!("{target}:8080")).unwrap()];
    let backend = isolation::backend(isolation::DOCKER).unwrap();
    let base = std::env::temp_dir().join("fleetify-net-test");
    let first = backend
        .set_up_network(&allow, &base)
        .expect("the first network starts");
    let second = backend
        .set_up_network(&allow, &base)
        .expect("the second network starts");

    let proxy_of_first = format!("{}-proxy", first.name);
    let address = docker(&[
        "inspect",
        "-f",
        "{{range .NetworkSettings.Networks}}{{.IPAddress}} {{end}}",
        &proxy_of_first,
    ]);
    let proxy_ip = address.split_whitespace().next().unwrap().to_string();
    let peer = agent(&second, DIRECT, &format!("{proxy_ip}:3128"));

    backend.tear_down_network(&second);
    backend.tear_down_network(&first);
    docker(&["rm", "-f", "fleetify-net-test-peer"]);
    assert!(
        peer.starts_with("error") || peer == "timeout",
        "the second run must not reach the first run's proxy, got {peer:?}"
    );
}

/// A network that a crashed run left behind: its owner process (id 99999999) does not exist, so
/// the sweep removes it. A live run's network would be kept, since its owner still runs.
#[test]
#[ignore]
fn a_crashed_runs_network_is_swept() {
    let name = "fleetify-99999999-1-0";
    docker(&[
        "network",
        "create",
        "--internal",
        "--label",
        "io.fleetify.network=1",
        name,
    ]);
    let removed = fleetify::container::sweep_orphans();
    assert!(
        removed.iter().any(|removed| removed == name),
        "the sweep must remove the orphaned network, removed {removed:?}"
    );
    let left = docker(&[
        "network",
        "ls",
        "--filter",
        &format!("name={name}"),
        "--format",
        "{{.Name}}",
    ]);
    assert!(
        left.is_empty(),
        "the orphaned network is still there: {left:?}"
    );
}

#[test]
#[ignore = "needs the docker daemon, node:22-bookworm-slim and fleetify-proxy:1"]
fn a_listed_name_that_resolves_to_a_private_address_is_refused() {
    // `localhost` is listed by name, and the proxy resolves it to its own loopback address.
    // The private-range rule must refuse it, since only listed addresses may be private.
    let allow = [parse_entry("localhost:8080").unwrap()];
    let backend = isolation::backend(isolation::DOCKER).unwrap();
    let base = std::env::temp_dir().join("fleetify-net-test");
    let network = backend
        .set_up_network(&allow, &base)
        .expect("the network starts");
    let fetched = agent(&network, FETCH, "http://localhost:8080/");
    let refused = backend.tear_down_network(&network);

    assert!(
        fetched.starts_with("error"),
        "a name that resolves to loopback is refused, got {fetched:?}"
    );
    assert!(
        refused.iter().any(|line| line.contains("localhost:8080")),
        "the refusal is logged as a denial: {refused:?}"
    );
}

#[test]
#[ignore = "needs the docker daemon, node:22-bookworm-slim and fleetify-proxy:1"]
fn an_unlisted_port_on_a_listed_address_is_refused_and_logged() {
    let allowed = upstream("fleetify-net-test-port");
    let allow = [parse_entry(&format!("{allowed}:8080")).unwrap()];
    let backend = isolation::backend(isolation::DOCKER).unwrap();
    let base = std::env::temp_dir().join("fleetify-net-test");
    let network = backend
        .set_up_network(&allow, &base)
        .expect("the network starts");

    let listed_port = agent(&network, VIA_PROXY, &format!("http://{allowed}:8080/"));
    let other_port = agent(&network, VIA_PROXY, &format!("http://{allowed}:9090/"));
    let refused = backend.tear_down_network(&network);
    docker(&["rm", "-f", "fleetify-net-test-port"]);

    assert_eq!(listed_port, "HTTP/1.1 200 OK", "the listed port answers");
    assert_eq!(
        other_port, "HTTP/1.1 403 Forbidden",
        "the same address on another port is refused"
    );
    assert!(
        refused
            .iter()
            .any(|line| line.contains(&format!("{allowed}:9090"))),
        "the refused port is logged: {refused:?}"
    );
}

#[test]
#[ignore = "needs the docker daemon, node:22-bookworm-slim and fleetify-proxy:1"]
fn a_request_to_the_proxy_itself_is_refused() {
    let allowed = upstream("fleetify-net-test-self");
    let allow = [parse_entry(&format!("{allowed}:8080")).unwrap()];
    let backend = isolation::backend(isolation::DOCKER).unwrap();
    let base = std::env::temp_dir().join("fleetify-net-test");
    let network = backend
        .set_up_network(&allow, &base)
        .expect("the network starts");
    let proxy_ip = docker(&[
        "inspect",
        "-f",
        &format!(
            "{{{{with index .NetworkSettings.Networks \"{}\"}}}}{{{{.IPAddress}}}}{{{{end}}}}",
            network.name
        ),
        &format!("{}-proxy", network.name),
    ]);

    // The gateway of the default bridge, where the proxy also sits: the host side of its route out.
    let gateway = docker(&[
        "network",
        "inspect",
        "bridge",
        "-f",
        "{{range .IPAM.Config}}{{.Gateway}}{{end}}",
    ]);
    let own_address = agent(&network, VIA_PROXY, &format!("http://{proxy_ip}:3128/"));
    let loopback = agent(&network, VIA_PROXY, "http://127.0.0.1:3128/");
    let host_gateway = agent(&network, VIA_PROXY, &format!("http://{gateway}:80/"));
    let refused = backend.tear_down_network(&network);
    docker(&["rm", "-f", "fleetify-net-test-self"]);

    assert_eq!(
        own_address, "HTTP/1.1 403 Forbidden",
        "the proxy's own address is refused"
    );
    assert_eq!(loopback, "HTTP/1.1 403 Forbidden", "loopback is refused");
    assert_eq!(
        host_gateway, "HTTP/1.1 403 Forbidden",
        "the gateway of the default bridge is refused"
    );
    assert!(
        refused.iter().any(|line| line.contains(&proxy_ip)),
        "the refusal of the proxy's own address is logged: {refused:?}"
    );
    assert!(
        refused.iter().any(|line| line.contains(&gateway)),
        "the refusal of the gateway is logged: {refused:?}"
    );
}
