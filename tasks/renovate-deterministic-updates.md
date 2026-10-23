# Idea: deterministic dependency updates with Renovate

**Status:** idea, not decided. The section in PLAN.md has the design; this file lists what must be settled first.

**Idea.** Simple dependency updates are done by a deterministic bot (Renovate). The LLM comes in only when CI fails, through the CI-fix loop, which is bounded by `fix_rounds`.

**Open points.**

1. **Branch ownership.** Renovate rebases its own branches and stops once a human has pushed to them. A fix pushed by the fix loop onto a Renovate branch may end Renovate's updates for that PR. Decide: fix on a separate branch with its own PR, or change Renovate's rebase setting.
2. **Who runs Renovate.** Renovate as a scheduled app or self-hosted job, or fleetify starts it per repository. First version proposal: Renovate runs on its own; fleetify observes the PRs and the CI.
3. **Scope of "simple".** Set by Renovate's own rules (for example patch and minor updates within the declared ranges, with green CI as the test), not by fleetify.
4. **Reporting.** A Renovate PR that fleetify did not create still needs a row in the report, with the same result names.
5. **Cost visibility.** The report shows how often a bot run and how often a fix round ran, so trial numbers stay honest.

**Done when.** The branch question is decided and written into DECISIONS.md, and a trial on one repository with one simple update ends as expected.
