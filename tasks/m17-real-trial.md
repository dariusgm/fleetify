# M17: real trial on three to five private repositories

**Status:** not started. Needs input from the user: the repositories and the task. `[ASK]` in the plan: nothing runs against real repositories without the go.

**Goal.** Measure whether fleetify brings value on real work: how many repositories go through without intervention, time per repository, where help was needed, and whether the failure blocks in the report were enough to act without opening the run folder.

**Input needed from the user.**

- Three to five private repositories, as `github:` or `gitlab:` lines.
- The task for each, as a recipe (or the task text, and fleetify drafts the recipe).
- Which agent and model.
- Whether CI should be waited for on each repository.

**Procedure.**

1. Dry run on all repositories, check the output.
2. Automated run (`--format json`), keep `report.json` and the event log.
3. Second run with `--ui` for the demonstration.
4. Evaluate from the report and the events; write the numbers into `TRIAL.md`.

**Done when.** `TRIAL.md` has the numbers per repository and a conclusion, and the open items the trial exposed are filed as tasks.

**Reference.** A reference run on a public fork ended in `error` (HTTP 404 on the CI dispatch). Resolve that first or record it as a known limit, see `fork-ci-triggers.md` and `bug-host-error-endpoint.md`.

## Decision after the trial

If most repositories go through without intervention, extending fleetify is worth it. If the result is thin, improve the prompt and the verifier first. If it stays thin, Armada or the SonarQube Remediation Agent may reach the goal faster than building further. The numbers from `report.json` and the event logs are the basis for that decision; write the decision into DECISIONS.md.

