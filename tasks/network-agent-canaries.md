# Network: canaries for each agent through the proxy

**Status:** in progress. Claude Code: done (2026-11-01, rootless: answer through the proxy with no refused request; the Django canaries of PR #44 not re-run). opencode: done (2026-11-01, rootless). Copilot: not started, see below.

**Per agent.** The canary runs the agent's real model call through the proxy with the recipe's allowlist, and checks that a request to an unlisted target is refused.

- **Claude Code:** done. 2026-10-31: positive and negative runs of the Django recipes (PR #44). 2026-11-01 under rootless: `tests/claude_network.rs`, a node probe and a live answer through the proxy with no refused request.
- **opencode:** done (2026-11-01), against the local llama.cpp server on `192.168.178.151:9931`. Tests in `tests/opencode_network.rs` and `tests/container.rs` (`opencode_in_its_container_answers_from_the_recipe_provider`). Details in `DECISIONS.md`, "Opencode through the proxy".
- **Copilot:** done (2026-11-01, rootless): `tests/copilot_network.rs`, a live answer through the proxy, with the token passed by name (`agent_env = ["GH_TOKEN"]`). The telemetry host is listed in the recipe's allow list, so no request is refused. Open: a node-level probe like `tests/opencode_network.rs`.

**Node note.** Node's `fetch` ignores proxy variables unless `NODE_USE_ENV_PROXY=1` is set. The network sets it. Any new Node-based agent needs the same probe, because a missing variable means direct access, and the probe is what catches it.

**Claude Code image.** Built on 2026-11-01. All six tests in `tests/container.rs` pass under rootless.

**Done when.** Each agent in `docker/agents/` has a passing live canary with its allowlist, a refused request is shown, and the result is in `DECISIONS.md`.
