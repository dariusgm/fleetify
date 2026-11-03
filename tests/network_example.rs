//! The `network-ignored-hosts` example: it validates, and the proxy configuration its recipe
//! produces has the rules the example documents (see its README.md). No docker needed.

use std::path::Path;
use std::process::Command;

use fleetify::egress::{parse_policy, squid_conf};
use fleetify::recipe::Recipe;

fn example_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/recipes/network-ignored-hosts")
}

#[test]
fn the_ignored_hosts_example_validates() {
    let output = Command::new(env!("CARGO_BIN_EXE_fleetify"))
        .arg("validate")
        .arg(example_dir())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("recipe network-ignored-hosts is valid"),
        "{stdout}"
    );
}

#[test]
fn the_example_produces_the_documented_proxy_rules() {
    let text = std::fs::read_to_string(example_dir().join("recipe.toml")).unwrap();
    let recipe = Recipe::parse(&text).unwrap();
    let policy = parse_policy(&recipe.network.allow, &recipe.network.ignore).unwrap();
    let conf = squid_conf(&policy, "172.30.0.0/16");

    // The allowed hosts are allowed; the ignored host is denied and its log line is filtered.
    assert!(conf.contains("acl target0 dstdomain api.anthropic.com\n"));
    assert!(conf.contains("http_access allow agents target0 port0\n"));
    assert!(conf.contains("acl target2 dstdomain telemetry.example.com\n"));
    assert!(conf.contains("http_access deny agents target2 port2\n"));
    assert!(conf.ends_with("access_log stdio:/dev/stdout !hidden2\n"));
    // The filter keeps the allowed hosts in the log.
    assert!(!conf.contains("!hidden0") && !conf.contains("!hidden1"));
    // The rejection comes before the catch-all.
    assert!(
        conf.find("http_access deny agents target2 port2").unwrap()
            < conf.find("http_access deny all").unwrap()
    );
}
