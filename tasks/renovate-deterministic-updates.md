# Deterministic updates: scripted changes and the Renovate bot

**Status:** partly done (2026-11-04). Scripted changes without an agent are done: a recipe with `[script]` runs a bash or PowerShell file, and the verifier, the guards and the CI decide (`DECISIONS.md`, "Scripted changes without an agent"). The Renovate bot is still open.

**Done.** The `[script]` table, the pipeline tests for a script-only run, the `scripted-update` example, the README section.

**Open (Renovate).**

1. **Branch ownership.** Renovate rebases its own branches and stops once a human has pushed to them. A fix pushed by the fix loop onto a Renovate branch may end Renovate's updates for that PR. Decide: fix on a separate branch with its own PR, or change Renovate's rebase setting.
2. **Who runs Renovate.** Renovate as a scheduled app or self-hosted job, or fleetify starts it per repository. First version proposal: Renovate runs on its own; fleetify observes the PRs and the CI.
3. **Reporting.** A Renovate PR that fleetify did not create still needs a row in the report, with the same result names.
4. **Cost visibility.** The report shows how often a bot run and how often a fix round ran, so trial numbers stay honest.

**Open (PowerShell).** The scripts run under PowerShell 7.6.9 on Linux (checked 2026-11-04, `DECISIONS.md`). Still open: one run on a Windows host through fleetify, since the windows command is picked only on Windows.
