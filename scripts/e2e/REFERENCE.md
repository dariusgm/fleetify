# E2E reference timings

Wall-clock times from real runs of `run_e2e.py`, for planning how long a run takes. The llama.cpp
server was on the same LAN; agent time depends on the model and the server load, so expect large
variation between runs.

## Run 10220552 (2026-10-22, llama.cpp `qwen3.8-flash-next`, e2e-manual-script)

| case | host | expected | got | setup s | run s | total s |
|------|------|----------|-----|--------:|------:|--------:|
| github-plain | github | pr_open | pr_open | 7 | 7 | 14 |
| github-verify-failed | github | verify_failed | verify_failed | 7 | 2 | 9 |
| github-opencode-llamacpp | github | pr_open | pr_open | 6 | 310 | 315 |
| github-opencode-llamacpp-ci | github | ci_passed | ci_passed | 7 | 350 | 357 |
| github-opencode-llamacpp-merge | github | merged | merged | 6 | 340 | 346 |
| gitlab-plain | gitlab | pr_open | pr_open | 4 | 9 | 12 |
| gitlab-verify-failed | gitlab | verify_failed | verify_failed | 6 | 3 | 9 |
| gitlab-opencode-llamacpp | gitlab | pr_open | pr_open | 4 | 245 | 249 |
| gitlab-opencode-llamacpp-ci | gitlab | ci_passed | ci_passed | 4 | 263 | 267 |
| gitlab-opencode-llamacpp-merge | gitlab | merged | merged | 3 | 311 | 315 |

All 10 cases matched. The GitHub CI cases ran this time, so the billing block from run 10211844 is
gone. The repositories were removed after the run.

Laufzeit pro Aufruf: the whole `run_e2e.py --yes` invocation took about 35 min wall clock (started
05:52, finished before 06:27 UTC). The ten cases add up to 1893 s (about 31.5 min) of setup and run;
the rest is the agent check at the start and the cleanup at the end.

## Run 10211844 (2026-10-21, llama.cpp `qwen3.8-flash-next`, main with rules v2)

| case | host | expected | got | setup s | run s | total s |
|------|------|----------|-----|--------:|------:|--------:|
| github-plain | github | pr_open | pr_open | 6 | 6 | 11 |
| github-verify-failed | github | verify_failed | verify_failed | 6 | 3 | 9 |
| github-opencode-llamacpp | github | pr_open | pr_open | 6 | 233 | 239 |
| github-opencode-llamacpp-ci | github | ci_passed | ci_failed (billing, see below) | 6 | 355 | 361 |
| github-opencode-llamacpp-merge | github | merged | ci_failed (billing) | 5 | 378 | 383 |
| gitlab-plain | gitlab | pr_open | pr_open | 8 | 57 | 65 |
| gitlab-verify-failed | gitlab | verify_failed | verify_failed | 8 | 8 | 16 |
| gitlab-opencode-llamacpp | gitlab | pr_open | pr_open | 5 | 279 | 285 |
| gitlab-opencode-llamacpp-ci | gitlab | ci_passed | ci_passed | 7 | 268 | 275 |
| gitlab-opencode-llamacpp-merge | gitlab | merged | merged | 4 | 336 | 340 |

Setup (create the repo and push) takes 4–8 s per repository. Plain and verify-failed cases take
seconds to about a minute; agent cases take 4–6 minutes on this llama.cpp server.

**Mismatch, not a fleetify fault:** both GitHub CI cases ended before any step ran. GitHub's job
annotation says: "The job was not started because recent account payments have failed or your spending
limit needs to be increased." The result is reported as `ci_failed` with the message "CI failed:
test", which hides the cause. Improving that message is a follow-up for fleetify.

## GitHub Actions on a public fork (observed 2026-10-22)

Observed on `dariusgm/icalendar`, a public fork of `collective/icalendar` with CI cut to one Linux
job and `ruff`. This is a real project's workflows, not a hello-world example.

- A push to the fork's `main` starts no workflow, although the workflow has `on: push`.
- A pull request inside the fork (branch to `main`) starts no workflow either.
- `gh workflow run tests.yml --ref <branch>` (`workflow_dispatch`) starts the run at once, on
  `main` and on the agent branch.
- A dispatched run creates its check-run on the commit SHA. A gate that reads check-runs by SHA
  sees it. `gh pr checks` on the PR still reports nothing.
- Cause not known. For the E2E cases on forks, dispatch the CI explicitly after the push, or check
  the Actions settings before relying on automatic triggers.
