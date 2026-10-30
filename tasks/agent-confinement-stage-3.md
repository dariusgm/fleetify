# Stage 3: narrowing what the coding agents may do

**Status:** started 2026-10-28, with the container part (below). The other points are not started. The first point of the scope, agents in containers, is built and its Claude Code canary passes.

**Scope.**

- Agents inside containers without credentials (required for company repositories). The agent gets only the workspace and its connection to the model.
- Per-agent network rules, so a verifier's child processes cannot reach hosts they do not need.
- Stricter file-system confinement, checked by the canary, with per-agent profiles (in the recipe, after the config-layer decision).
- Event-stream agent adapters, so the agent's actions show up as events, not only as log lines.

**Done when.** Each point has a canary test that fails on the unconfined behaviour, and the result is in DECISIONS.md.

## Progress (2026-10-28)

- **Done: agents in containers, opt-in per recipe.** `image` in `recipe.toml`; per-agent images in `docker/agents/`; orchestrator image in the root `Dockerfile`. Details and limits are in `DECISIONS.md` ("Agent containers") and the README's "Containers" section.
- **Done with tests:** same-path mounts, names-only environment, read-only model login, image label check, `on_stop` cleanup. `tests/container.rs` runs against the daemon and is ignored by default.
- **Done: live canary for Claude Code in its container** passed after the host login was renewed. Copilot and opencode are not checked yet, and need their login passed in first.
- **Not started:** the per-agent network rules, stricter file-system profiles per agent, event-stream adapters, and a credential-free agent (the model login stays in the container).

## Plan: network isolation, docker backend (2026-10-30)

Decisions are in `DECISIONS.md` ("Stage-3 network design" and the proxy-denials entry).

**Status: steps 1 to 7 are built and tested against the daemon (2026-10-30). Step 8 is open.** The live canary through the proxy is still needed for Claude Code, opencode and Copilot. It needs a model endpoint and a login, so it has not been run.

1. **Recipe.** `[network] allow = ["host:port", ...]`, needs `image`, required when `image` is set. Entries: a DNS name or an IPv4 address, plus a port. No CIDR, wildcard, scheme or path. Refused by entry, with the reason.
2. **Policy module `src/egress.rs`.** Parses the entries. Writes the Squid configuration for one run (ACL per entry, the agent subnet as the only client, `positive_dns_ttl` short, `via off`, `forwarded_for delete`, deny otherwise). Reads the Squid access log and returns the `TCP_DENIED` targets.
3. **Proxy image `docker/proxy/`.** Debian slim with Squid, `USER proxy`, runs in the foreground with the generated config mounted read-only. Labelled `io.fleetify.role=egress-proxy`.
4. **Isolation layer.** `Backend` gets `set_up_network` and `tear_down_network`. The `Spec` gets an optional network. The docker backend: `docker network create --internal` with a label, read the subnet back, write the config to a temporary directory outside the workspace, start the proxy on the default bridge and connect it to the internal network, wait until it accepts connections. Tear-down: read the proxy's log for denials, remove the proxy, the network and the directory.
5. **Agent container.** `run_argv` adds `--network <internal>`. The agent's environment gets `HTTPS_PROXY`, `HTTP_PROXY` and their lowercase forms, pointing at the proxy's name. No `NO_PROXY`.
6. **Pipeline.** Set-up before the agent's run, tear-down after it, whatever the outcome. Each denial becomes an event (`NetworkDenied`) for the repository.
7. **Tests.** Unit: entry rules, Squid text, denial parsing, the `--network` argument, the proxy variables. Integration (ignored by default, needs the daemon): an upstream container on the bridge, one listed by address and one not. Checks: listed succeeds, unlisted gets 403 and a denial line, the agent container publishes no port, two networks cannot reach each other.
8. **Not in this step.** Orphan sweep for crashed runs, denials in `report.json`, the live model canary through the proxy, and the Copilot and opencode canaries. `check-agent` still runs without the network. That is a gap, and the live canary covers it.
