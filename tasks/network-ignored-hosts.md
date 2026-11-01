# Network: ignored hosts (rejected, not logged, not shown)

**Status:** open, idea agreed, not built.

**Idea.** Some hosts an agent contacts are not needed for its work and should not be allowed, for example a vendor's telemetry server. Listing them in `[network] allow` would allow them, which is the wrong meaning. Leaving them out makes every run show a refusal, which is noise in the events and the log. A third list, `ignore`, solves both: a request to an ignored host is rejected by the proxy, and nothing is logged or shown for it.

**Open.**

1. **Recipe field.** `[network] ignore = ["host:port", ...]`, same entry rules as `allow` (`src/egress.rs`). An entry in both lists is refused by validation.
2. **Proxy.** Squid configuration: a `deny` rule for each ignored target, before the allow rules, with the access-log line suppressed for that ACL (Squid's `access_log` supports ACL filters; check the version pinned in `docker/proxy/`). The connection still fails for the agent.
3. **Denials.** `denial_counts` in `src/egress.rs` and the `NetworkDenied` events do not include ignored targets. Test: an ignored request fails for the agent, and its target is absent from the log, the events and the report.
4. **First use.** Telemetry hosts, for example Copilot's `telemetry.individual.githubcopilot.com`, once the user decides which hosts are telemetry rather than model or work traffic.

**Done when.** A test shows an ignored host is rejected, not in the log, not in the events, and an allowed host next to it works. `DECISIONS.md` records the list semantics.
