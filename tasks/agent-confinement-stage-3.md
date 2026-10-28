# Stage 3: narrowing what the coding agents may do

**Status:** started 2026-10-28, with the container part (below). The other points are not started. The first point of the scope, agents in containers, is built; its live canary is blocked on an expired Claude login.

**Scope.**

- Agents inside containers without credentials (required for company repositories). The agent gets only the workspace and its connection to the model.
- Per-agent network rules, so a verifier's child processes cannot reach hosts they do not need.
- Stricter file-system confinement, checked by the canary, with per-agent profiles (in the recipe, after the config-layer decision).
- Event-stream agent adapters, so the agent's actions show up as events, not only as log lines.

**Done when.** Each point has a canary test that fails on the unconfined behaviour, and the result is in DECISIONS.md.

## Progress (2026-10-28)

- **Done: agents in containers, opt-in per recipe.** `image` in `recipe.toml`; per-agent images in `docker/agents/`; orchestrator image in the root `Dockerfile`. Details and limits are in `DECISIONS.md` ("Agent containers") and the README's "Containers" section.
- **Done with tests:** same-path mounts, names-only environment, read-only model login, image label check, `on_stop` cleanup. `tests/container.rs` runs against the daemon and is ignored by default.
- **Blocked:** the live canary for Claude Code in its container. The Claude login on the host has expired, so the agent cannot reach its model, in or out of the container. Renew the login, then run `fleetify check-agent claude-code --image fleetify-agent-claude-code`.
- **Not started:** the per-agent network rules, stricter file-system profiles per agent, event-stream adapters, and a credential-free agent (the model login stays in the container).
