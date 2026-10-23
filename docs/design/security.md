# Security (minimum)

> Design of fleetify. Moved from the former PLAN.md when it was dissolved (2026-10-23). Describes the target design; open work is in `tasks/`.

- Push, PR/MR, CI polling and merge are done only by the orchestrator. The agent gets no host tokens. In the agent's workspace `remote.origin.pushurl` is set to an invalid value; the orchestrator pushes via the explicit URL. That is a hurdle, not a sandbox.
- The agent process gets an environment allowlist, not the user's full environment.
- Agents are started with explicit permissions: workspace only, allowlisted commands (see "Permissions and workspace restriction"). An agent and version without a passing canary check triggers a warning; with `require_checked_agent = true` it is refused unless `--allow-unchecked-agent` is set.
- Agent CLIs often run with broad permissions (auto-approve). Acceptable for private repos, for company repos only inside a container without credentials (stage 2).
- Inputs from repos (branch names, file names, scanner messages) never flow into shell strings.
- Every process has a timeout; on expiry the whole process tree is terminated.
- Everything the agent wrote (including `result.md`) is untrusted data: displayed and length-limited in the report, never executed or interpreted as instructions.

