# Copilot in containers: a token that can only call Copilot

**Status:** done (2026-11-02). The Copilot container gets a token that can only call Copilot, and nothing else in fleetify uses the broad `gh` token. The decision and the checks are in `DECISIONS.md`.

**What was done.**

- The token is a fine-grained personal access token with the account permission Copilot Requests (read-only) and no other permission. Private repositories return 404 with it, and Copilot answers.
- It is named `COPILOT_GITHUB_TOKEN`, the variable Copilot checks first. It is passed into a Copilot container by name (`agent_env`) and nowhere else. A Copilot container does not start without it, and check-agent needs it too.
- `tests/copilot_network.rs` reads that variable. No test, recipe or example uses the `gh` token.
- The README documents the name, how to create the token, the three-day renewal, and the export.

**Still operational.** The token expires after three days. Renewing it, and revoking the old one, is the owner's step. The value was pasted into a chat during the work, so it is in that transcript: revoke it when the test window closes.

**Not part of this task.** Keeping the token out of the container entirely is the broker, in `tasks/copilot-broker.md`.
