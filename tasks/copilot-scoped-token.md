# Copilot in containers: a token that can only call Copilot

**Status:** partly done (2026-11-02). A Copilot-only token is in use for the test: a fine-grained token with Copilot Requests (read-only) only, verified: private repositories return 404. The test uses `COPILOT_GITHUB_TOKEN`. Open: rotation (3-day token), and the broker (step 5). See `DECISIONS.md`.

**Known.** The Copilot container test (`tests/copilot_network.rs`) passes the `gh` token into the container as `GH_TOKEN`. Copilot accepts it and answers. The `gh` token belongs to the account `dariusgm` and has the scopes `gist`, `read:org`, `repo` and `workflow`. Inside the container the agent could use it against every repository of the account. That is far more than Copilot needs, and the test must not be copied into a real recipe with this token.

Copilot's own login on the host is separate (kept in its keyring, not in `~/.copilot/config.json`). The container cannot reach it.

**Open.**

1. Check whether GitHub offers a fine-grained personal access token with a Copilot permission (for example "Copilot Requests") that works without any repository access, and whether this account and plan allow it.
2. Create such a token and check it: Copilot answers `pong` in the container with the token; the same token is refused by the GitHub API for a repository (`gh api repos/<owner>/<repo>` with that token must fail).
3. Document the token's name and expiry in the recipe guidance, and pass it through `agent_env = ["GH_TOKEN"]` only. The value stays in the orchestrator's environment, never in the recipe.
4. Decide whether the test uses the scoped token by default (`COPILOT_TEST_TOKEN`, separate from `GH_TOKEN`), so the broad token is not used by accident.
5. Long term: a broker that keeps the Copilot login outside the container (`DECISIONS.md`, the agent-login entries).

**Done when.** A scoped token is documented and tested, `tests/copilot_network.rs` runs with it, and the broad `gh` token is no longer used by any test or recipe example. The result goes into `DECISIONS.md`.
