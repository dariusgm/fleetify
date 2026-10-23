# Working agreement

> Rules for working on fleetify (human or agent). Moved from the former PLAN.md (2026-10-23).

- Everything in English: identifiers, comments, commit messages, docs, user-facing text.
- One branch per milestone (`m01-command-runner`, …), small commits, tests first for guards, credential matching, command building, report building and idempotency. A milestone is done when its acceptance criteria pass on both OS in CI and `clippy -D warnings` is clean.
- Check each agent CLI's and each host API's current documentation before relying on flags or endpoints; note the documentation URL and date in the adapter's module doc comment.
- Ask before every step marked `[ASK]`: anything that touches real repos, real agents or a merge.
- Never write secrets into logs, events, task files, state files, PR bodies or the report.
- Every new failure path must produce a `failure` block in the report with a non-empty message; a test asserts it.
- When a design question is not answered by this plan, choose the most conservative option, note it in `DECISIONS.md` and continue; do not block on it.

