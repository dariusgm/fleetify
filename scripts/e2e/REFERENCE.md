# E2E reference timings

Wall-clock times from real runs of `run_e2e.py`, for planning how long a run takes. The llama.cpp
server was on the same LAN; agent time depends on the model and the server load, so expect large
variation between runs.

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
