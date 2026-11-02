# Network: canaries from the design that have no test

**Status:** partly done (2026-11-02). Three of four checks are in `tests/network.rs`, and all six network tests pass under rootless Docker. The DNS-change check is open: it needs a DNS server the proxy can reach, which the test does not have yet.

**Done.**

- **Unlisted port on a listed address** (`an_unlisted_port_on_a_listed_address_is_refused_and_logged`): the same address on another port returns 403 and is logged.
- **The proxy's own requests to the gateway** (`a_request_to_the_proxy_itself_is_refused`): an agent's request to the proxy's own address, to loopback and to the bridge gateway returns 403 and is logged. These are requests from the agent to the proxy's network, which is what the private-address rule governs. Requests made by the proxy process itself are not in the test.
- **Denied address, not name:** already covered by `the_agent_reaches_only_listed_targets_and_the_refusals_are_reported`, which refuses the upstream's container address. The earlier note that the test refused a name only was wrong.

**Open.**

1. **A listed name's record changes mid-run.** The next request must follow the new address after `positive_dns_ttl` (60 s). Tried and not working: writing the record into the proxy's `/etc/hosts`. Squid resolves names through DNS and does not read `/etc/hosts` for them (`localhost` works only because the name is special). The test needs a DNS server the proxy resolves through. Options: a small DNS responder in the agent image on the internal network, with the proxy's resolver pointed at it inside the test only. That must not become a configuration option (the user decided against custom DNS settings). Until then this check stays open, and the TTL value is covered by the unit test of the Squid text only.

**Done when.** The DNS-change check exists in `tests/network.rs`, is run under rootless Docker, and passes. `DECISIONS.md` records the result.
