//! The network policy of an agent container (stage 3): which `host:port` entries the agent may
//! reach through its proxy, the Squid configuration that enforces them, and the reading of the
//! proxy's access log for refused requests. See `DECISIONS.md`, "Stage-3 network design".
//!
//! The proxy only sees the agent's requests. It never terminates TLS: an HTTPS request is a
//! CONNECT to `host:port`, which is what the rules match, so no certificate is involved.

use std::net::Ipv4Addr;

/// The port the proxy listens on inside its container.
pub const PROXY_PORT: u16 = 3128;

/// How long a name's address is kept, in seconds. Squid's default is six hours, which would
/// send a multi-day run to an old address after a DNS change.
pub const DNS_TTL_S: u32 = 60;

/// The host part of one entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A DNS name, matched exactly and looked up again per connection.
    Name(String),
    /// An IPv4 address, for a host that is configured by address.
    Address(Ipv4Addr),
}

/// One allowed `host:port`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub target: Target,
    pub port: u16,
}

/// Parses one `host:port` entry. Refuses ranges, wildcards, schemes, paths, IPv6 and a
/// missing or zero port, and says why.
pub fn parse_entry(text: &str) -> Result<Entry, String> {
    if text.contains('/') || text.contains('*') || text.contains("://") {
        return Err(format!(
            "{text:?}: ranges, wildcards, schemes and paths are not allowed; list each host and port"
        ));
    }
    let (host, port) = text
        .rsplit_once(':')
        .ok_or_else(|| format!("{text:?} needs a port, as host:port"))?;
    if host.contains(':') || host.contains('[') {
        return Err(format!("{text:?}: IPv6 addresses are not supported yet"));
    }
    let port = port
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or_else(|| format!("{text:?}: the port must be 1 to 65535"))?;
    let target = match host.parse::<Ipv4Addr>() {
        Ok(address) => Target::Address(address),
        Err(_) if valid_name(host) => Target::Name(host.to_ascii_lowercase()),
        Err(_) => {
            return Err(format!(
                "{text:?}: {host:?} is not a DNS name or an IPv4 address"
            ));
        }
    };
    Ok(Entry { target, port })
}

/// Whether `name` is a DNS name: dot-separated labels of letters, digits and hyphens, none
/// starting or ending with a hyphen, and not ending in digits (that is an address, not a name).
fn valid_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 253 {
        return false;
    }
    let labels: Vec<&str> = name.split('.').collect();
    let shaped = labels.iter().all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    });
    let last_is_numeric = labels
        .last()
        .is_some_and(|label| label.chars().all(|c| c.is_ascii_digit()));
    shaped && !last_is_numeric
}

/// Parses every entry of a recipe's allowlist, in order.
pub fn parse_entries(texts: &[String]) -> Result<Vec<Entry>, String> {
    texts.iter().map(|text| parse_entry(text)).collect()
}

/// The Squid configuration for one run. Only clients in `agents` (the subnet of the agent's
/// internal network) may use the proxy, and only the listed targets and ports are allowed.
/// Everything else is denied and logged.
pub fn squid_conf(entries: &[Entry], agents: &str) -> String {
    let mut text = format!(
        "# Written by fleetify for one agent run. Do not edit.\n\
         http_port {PROXY_PORT}\n\
         pid_filename /tmp/squid.pid\n\
         cache deny all\n\
         cache_log /dev/stderr\n\
         access_log stdio:/dev/stdout\n\
         dns_v4_first on\n\
         positive_dns_ttl {DNS_TTL_S} seconds\n\
         negative_dns_ttl 5 seconds\n\
         forwarded_for delete\n\
         via off\n\
         acl agents src {agents}\n\
         http_access deny !agents\n"
    );
    for (index, entry) in entries.iter().enumerate() {
        let target = match &entry.target {
            Target::Name(name) => format!("acl target{index} dstdomain {name}"),
            Target::Address(address) => format!("acl target{index} dst {address}"),
        };
        text.push_str(&format!(
            "{target}\nacl port{index} port {}\nhttp_access allow agents target{index} port{index}\n",
            entry.port
        ));
    }
    text.push_str("http_access deny all\n");
    text
}

/// The requests the proxy refused, as `METHOD target`, from its access log. Squid writes one
/// line per request in its default format: time, elapsed, client, status, size, method, URL.
pub fn denied(access_log: &str) -> Vec<String> {
    access_log
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let status = fields.get(3)?;
            let method = fields.get(5)?;
            let url = fields.get(6)?;
            status
                .starts_with("TCP_DENIED")
                .then(|| format!("{method} {url}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_addresses_with_a_port_are_accepted() {
        assert_eq!(
            parse_entry("API.Example.com:443").unwrap(),
            Entry {
                target: Target::Name("api.example.com".to_string()),
                port: 443
            }
        );
        assert_eq!(
            parse_entry("10.0.0.20:8080").unwrap(),
            Entry {
                target: Target::Address(Ipv4Addr::new(10, 0, 0, 20)),
                port: 8080
            }
        );
    }

    #[test]
    fn ranges_wildcards_schemes_paths_ipv6_and_bad_ports_are_refused() {
        for text in [
            "10.0.0.0/24:443",
            "*.example.com:443",
            "https://example.com:443",
            "example.com:443/v1",
            "[::1]:8080",
            "example.com",
            "example.com:0",
            "example.com:70000",
            "example.com:",
            "bad_name:443",
            "-bad.example.com:443",
            "10.0.0:443",
        ] {
            assert!(parse_entry(text).is_err(), "{text} should be refused");
        }
    }

    #[test]
    fn the_config_allows_only_listed_targets_for_the_agent_subnet() {
        let entries = [
            parse_entry("api.example.com:443").unwrap(),
            parse_entry("10.0.0.20:8080").unwrap(),
        ];
        let text = squid_conf(&entries, "172.30.0.0/16");
        assert!(text.contains("acl agents src 172.30.0.0/16\nhttp_access deny !agents\n"));
        assert!(text.contains("acl target0 dstdomain api.example.com\n"));
        assert!(text.contains("acl port0 port 443\n"));
        assert!(text.contains("acl target1 dst 10.0.0.20\n"));
        assert!(text.contains("http_access allow agents target1 port1\n"));
        assert!(text.contains("positive_dns_ttl 60 seconds\n"));
        assert!(text.ends_with("http_access deny all\n"));
    }

    #[test]
    fn denied_requests_are_read_from_the_access_log() {
        let log = "\
1700000000.1 0 172.30.0.5 TCP_DENIED/403 3803 CONNECT evil.example:443 - HIER_NONE/- text/html
1700000000.2 5 172.30.0.5 TCP_TUNNEL/200 39 CONNECT api.example.com:443 - HIER_DIRECT/1.2.3.4 -
1700000000.3 0 172.30.0.5 TCP_DENIED/403 3803 GET http://10.9.9.9:80/x - HIER_NONE/- text/html
";
        assert_eq!(
            denied(log),
            ["CONNECT evil.example:443", "GET http://10.9.9.9:80/x"]
        );
    }
}
