# Verifier: three gates

> Design of fleetify. Moved from the former PLAN.md when it was dissolved (2026-10-23). Describes the target design; open work is in `tasks/`.

1. **Fast verifier (inner loop, runs in the agent).** One command, named in `task.md`, that the agent runs itself after each change. It runs from the copy under `.agent/tools/` inside the workspace, because a confined agent cannot execute files outside it. `{tools}` resolves to the workspace copy in the inner loop and to the trusted original in the outer gate. The agent can tamper with its copy, which is harmless because the outer gate is authoritative. The verifier's own child processes (e.g. package caches) may still need paths outside the workspace; test that. It must be fast (formatting, lint, unit tests, quick static checks). Output shown to the agent is compact: exit code, few lines on success, on failure the lines matching `error_patterns` plus the last `tail_lines`, so the agent's context is not flooded. The full output goes to `verify-<n>.log`.
2. **Outer gate (orchestrator).** After the agent ends, the same verifier runs again from the trusted copy (because the agent can edit files in the repo, including a verifier script inside it), then the guards run. Only if both pass does anything get pushed.
3. **CI gate.** Integration tests run in the repository's own CI on the pushed commit. The gate is defined in "CI layer and CI gate" below. **Merging starts only after CI is green on the exact commit that was pushed.**

**Merge is a two-key operation and off by default:** the recipe must set `merge = "when_green"` *and* the run must be started with `--merge`. The merge is done by the orchestrator through the host API and respects branch protection and approval rules. If the host refuses, the repo ends as `merge_blocked`; fleetify never forces a merge. Without both keys the run ends at `pr_open` (no CI wait) or `ci_passed` (CI green) and a human merges.

**Delivery ladder (opt-in, one step at a time).** Private repositories get the most conservative setting by default; each further step has to be asked for in the recipe, and the steps are validated against each other:

1. `delivery.open_mr = false`: push the branch and stop. Result `pushed`. Nothing is opened, nothing is merged, nothing is deleted.
2. `delivery.open_mr = true` (default): push and open the pull or merge request. Result `pr_open`. A human merges.
3. `ci.merge = "when_green"` (needs `ci.wait`, so it needs M11) together with `--merge` at run time: merge after CI is green on the pushed commit, with the expected-head guard.
4. `delivery.delete_branch = true` (needs step 3): delete the source branch, but only after the merge was confirmed. Not deleted on any other result.

Each step is refused at validation if the step before it is off: `when_green` needs `open_mr`, and `delete_branch` needs `when_green`. A push-only run that finds its branch already on the remote ends as `already_done`, so it never pushes or opens anything over an earlier run.

Stage 1 only reports CI failures. Feeding a CI failure back to the agent as a new attempt (CI-fix loop) is stage 2.

