# Stage 3: narrowing what the coding agents may do

**Status:** started 2026-10-28. Two points are built: agents in containers (Claude Code canary passes) and per-recipe network rules through a proxy (built on 2026-10-30; negative test on 2026-10-31). The positive e2e canary through the proxy ran for Claude Code on 2026-10-31. The orphan sweep is built (2026-10-31). Open: the other agents' canaries, file-system profiles, event-stream adapters and a credential-free agent.

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
- **Not started:** stricter file-system profiles per agent, event-stream adapters, and a credential-free agent (the model login stays in the container).
- **Network rules: built as one allowlist per recipe** (`[network] allow`), not per agent. `DECISIONS.md` ("Stage-3 network design") records why. The allowlist is the one scope point the first version of this file called per-agent.

## Plan: network isolation, docker backend (2026-10-30)

Decisions are in `DECISIONS.md` ("Stage-3 network design" and the proxy-denials entry).

**Status: steps 1 to 7 are built and tested against the daemon (2026-10-30). Step 8 is partly open.**

- **Negative test, done (2026-10-31).** `examples/recipes/network-allowlist-denied/` names the model host wrongly (`api.anthropic.co`). The Claude Code agent's model request is refused by the proxy, the run ends with `error`, and nothing is pushed. The refusals are in the run's log with the target, the reason and the count (added 2026-10-31; before that they were only in the live output).
- **Positive e2e canary, done (2026-10-31) for Claude Code.** `examples/recipes/network-allowlist-permitted/` runs the agent in `fleetify-agent-claude-code-py310` with `api.anthropic.com`, `github.com`, `pypi.org` and `files.pythonhosted.org` allowed. Run `positive-canary-py310` ended with `pr_open` (`dariusgm/fleetify-django-hello` PR #2, a `/health` endpoint and test; the Django tests pass). The log shows only Claude Code's own Datadog and `mcp-proxy.anthropic.com` requests as refused. Allowed requests are not logged, so the evidence is the completed run through the proxy. The recipe and image are in fleetify PR #44 (`test/network-positive-canary`).
- **Not yet covered:** opencode and Copilot through the proxy, and `check-agent` with the recipe's network (it still runs without one).

1. **Recipe.** `[network] allow = ["host:port", ...]`, needs `image`, required when `image` is set. Entries: a DNS name or an IPv4 address, plus a port. No CIDR, wildcard, scheme or path. Refused by entry, with the reason.
2. **Policy module `src/egress.rs`.** Parses the entries. Writes the Squid configuration for one run (ACL per entry, the agent subnet as the only client, `positive_dns_ttl` short, `via off`, `forwarded_for delete`, deny otherwise). Reads the Squid access log and returns the `TCP_DENIED` targets.
3. **Proxy image `docker/proxy/`.** Debian slim with Squid, `USER proxy`, runs in the foreground with the generated config mounted read-only. Labelled `io.fleetify.role=egress-proxy`.
4. **Isolation layer.** `Backend` gets `set_up_network` and `tear_down_network`. The `Spec` gets an optional network. The docker backend: `docker network create --internal` with a label, read the subnet back, write the config to a temporary directory outside the workspace, start the proxy on the default bridge and connect it to the internal network, wait until it accepts connections. Tear-down: read the proxy's log for denials, remove the proxy, the network and the directory.
5. **Agent container.** `run_argv` adds `--network <internal>`. The agent's environment gets `HTTPS_PROXY`, `HTTP_PROXY` and their lowercase forms, pointing at the proxy's name. No `NO_PROXY`.
6. **Pipeline.** Set-up before the agent's run, tear-down after it, whatever the outcome. Each denial becomes an event (`NetworkDenied`) for the repository.
7. **Tests.** Unit: entry rules, Squid text, denial parsing, the `--network` argument, the proxy variables. Integration (ignored by default, needs the daemon): an upstream container on the bridge, one listed by address and one not. Checks: listed succeeds, unlisted gets 403 and a denial line, the agent container publishes no port, two networks cannot reach each other.
8. **Not done.** Denials in `report.json` (they are in the run log and events, not the report), live streaming of denials (they are read when the run ends), and the Copilot and opencode canaries through the proxy. `check-agent` still runs without the network.
