# Stage 3: narrowing what the coding agents may do

**Status:** not scheduled. Starts after the CI-fix loop has run on real work.

**Scope.**

- Agents inside containers without credentials (required for company repositories). The agent gets only the workspace and its connection to the model.
- Per-agent network rules, so a verifier's child processes cannot reach hosts they do not need.
- Stricter file-system confinement, checked by the canary, with per-agent profiles (in the recipe, after the config-layer decision).
- Event-stream agent adapters, so the agent's actions show up as events, not only as log lines.

**Done when.** Each point has a canary test that fails on the unconfined behaviour, and the result is in DECISIONS.md.
