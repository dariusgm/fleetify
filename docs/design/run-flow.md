# Run flow per repo

> Design of fleetify. Moved from the former PLAN.md when it was dissolved (2026-10-23). Describes the target design; open work is in `tasks/`.

1. Clone or fetch the cache clone, resolve the default branch, record its SHA as the base commit.
2. `applies_to` false (evaluated on the cache clone): result `skipped`.
3. Idempotency check against the host:
   - open PR/MR for the branch exists → `already_done`, except: if `ci.wait` is set and this run's `state.json` has no CI result for that PR's head, resume at step 10 (CI gate) for that head.
   - remote branch exists but no open PR/MR (e.g. a crash between push and PR creation) → resume at step 9b: open the PR/MR on the existing branch, no new commit.
   - this run's `state.json` shows a finished or interrupted workspace for this repo → the workspace is deleted and the repo starts over at step 4 (the agent's attempt is not reused in stage 1).
4. Create the workspace (`git clone --local` from the cache, `origin` = real remote, invalid `pushurl`, `.agent/` excluded), create the branch from the base commit.
5. `prepare`, if given.
6. If `instructions` exist: copy `tools` and attachments into `.agent/`, write `.agent/task.md`, start the agent (stdin closed), stream output, watch idle time and timeout, watch for prompts and `blocked.md`. Collect `result.md` / `blocked.md`.
7. No changes against the base commit: result `no_changes`.
8. Outer gate: verifier (trusted copy) and guards (and judge, if enabled).
9. (a) Single commit made by the orchestrator, push via `GIT_ASKPASS`. (b) Open PR/MR against the default branch. The PR body names the recipe, includes the verifier summary and the agent's `result.md` (clearly marked as agent-written, length-limited). Without `ci.wait`: result `pr_open`.
10. CI gate for the pushed commit (see "CI layer and CI gate"). Green without merge: `ci_passed`.
11. Optional merge (two keys): `merged` or `merge_blocked`.
12. Append the final result to `results.jsonl`; when the last repo finishes, write `report.json`.

Phases: `queued, cloning, preparing, agent, verifying, pushing, ci, merging`. Final results: `pr_open, ci_passed, merged, skipped, already_done, no_changes, verify_failed, guard_violation, needs_review, needs_input, blocked, agent_timeout, ci_failed, ci_missing, ci_blocked, ci_timeout, merge_blocked, interrupted, error`.

**Successful results** (used by `--continue-if-green` and the exit code): `pr_open` when `ci.wait` is off, `ci_passed`, `merged`, `skipped`, `already_done`, `no_changes`. Everything else counts as not successful.

