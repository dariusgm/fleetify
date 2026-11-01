# Network: canaries from the design that have no test

**Status:** open. Each item below is in the design (`DECISIONS.md`, "Stage-3 network design", Canary) and not in `tests/network.rs` or the unit tests.

**Missing checks.**

1. **Listed name's DNS record changes mid-run.** The next request must follow the new address. Test: change a name's record in a test resolver or `/etc/hosts` of the proxy, then send a request. `positive_dns_ttl` is set to 60 s in `src/egress.rs`; the test should show the change is picked up within that window.
2. **Unlisted port on a listed host is refused.** Only the ACL text is checked in a unit test (`src/egress.rs`). Add an integration check: the listed host on another port returns 403 and the denial is logged.
3. **The proxy's own requests to the host gateway fail.** Depends on `network-resolved-address-refusal.md`. Test from the proxy's side (for example `docker exec` into the proxy and connect to the gateway).
4. **Denied IP fails, by address, not by name.** Current test refuses a name only. Add an unlisted address case.

**Done when.** Each check exists in `tests/network.rs` (or a unit test), fails against the configuration before the fix where the design requires a refusal, and passes after it.
