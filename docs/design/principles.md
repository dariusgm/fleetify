# Principles

> Design of fleetify. Moved from the former PLAN.md when it was dissolved (2026-10-23). Describes the target design; open work is in `tasks/`.

1. **The orchestrator owns every side effect:** clone, branch, commit, push, PR/MR, CI polling, merge, tokens. The agent only edits files in its workspace and runs the verifier.
2. **Three gates, from fast to authoritative:** (a) fast verifier inside the agent loop, (b) the same verifier plus guards as the outer gate before anything is pushed, (c) CI on the PR/MR as the last gate before any merge.
3. **Observable by default, report first.** Every run emits one typed event stream. The event log, the state files, the report and every renderer are built from it. The automated caller gets exit code plus `report.json`; the live view is optional sugar.
4. **Never wait for a human inside a run.** Agents run non-interactively. If an agent needs input, the run ends as `needs_input` or `blocked` and the workspace is kept.
5. **Idempotent re-runs.** Running the same command again skips what is already done and resumes what was interrupted (rules under "Run flow per repo").
6. **No implicit shell.** Commands are argument lists. Shell, OS and quoting are explicit.
7. **No logic in the recipe format.** A recipe is data. Anything that needs logic is a script in the recipe's `tools/` directory, started by the orchestrator with timeouts and logging like every other command.
8. **fleetify owns the agent invocation, including its permissions** (which actions the agent may take, which directory it may touch). Provider, model and login stay in the agent's own configuration. What an agent does inside the granted permissions is the responsibility of the agent and its vendor. Permissions are defined in `config.toml` with restrictive defaults; anything beyond the defaults is granted by the user and is at the user's own risk.

