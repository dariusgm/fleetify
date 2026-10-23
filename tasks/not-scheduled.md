# Not scheduled

Items that are neither the CI loop nor agent confinement. Each one needs a reason before it is scheduled.

- SQLite instead of files; per-repository attempts; `retry` and `skip` with reasons; `takeover`; reusing an interrupted agent attempt.
- Rollback of created change requests (close the PR or MR, delete the branch, or revert after a merge), driven by a registry of created artifacts.
- More CI providers (GitHub Actions, Jenkins via commit statuses), job-level policies, retries of flaky jobs.
- Campaign time limits, circuit breaker, rate-limit governor, event-stream adapters for agents.
- Tokens from the OS credential store; optional CLI variants of the host layer.
- Judge evaluation over time, failure classes, webhook on `needs_human`.
- An embedded scripting language (Rhai) for `applies_to`, only if the condition list proves insufficient.
- `verify` as an MCP tool, plan SDK with JSON plans, broadcast shell, web UI, statistics per recipe version (see `PLAN-ausbau.md`).
