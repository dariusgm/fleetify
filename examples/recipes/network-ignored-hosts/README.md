# Example: ignored hosts

The recipe lists the model host and GitHub in `allow`, and a telemetry host in `ignore`.

```toml
[network]
allow = ["api.anthropic.com:443", "github.com:443"]
ignore = ["telemetry.example.com:443"]
```

Expected behaviour, which the tests check:

- A request to an allowed host goes through, and is logged.
- A request to the ignored host is rejected by the proxy with 403. The agent sees a failed connection.
- The ignored request is not in the proxy's log and not in the run's events or report.
- A port of an ignored host that is not in `allow` is refused with no log line. Silence is by host; the rejection is by host and port (see `DECISIONS.md`, "Ignored hosts").
- An entry in both `allow` and `ignore` is refused by `fleetify validate`.

The rules are checked by `tests/validate_cli.rs` (the recipe validates and its proxy configuration has the documented rules) and by `tests/network.rs` (`an_ignored_host_is_rejected_and_not_logged`, against the real proxy).

`repos.txt` and `tools/` are the same as in `network-allowlist-denied`; the example is for reading, not for a run against a real repository.
