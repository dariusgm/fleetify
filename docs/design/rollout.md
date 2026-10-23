# Rollout in waves (ramp-up)

> Design of fleetify. Moved from the former PLAN.md when it was dissolved (2026-10-23). Describes the target design; open work is in `tasks/`.

Real runs over more than one repo go in waves. Default: `--waves 1,all`.

- Wave 1 is one repo (or `--canary-repos a,b` to choose representative ones). It runs through all configured gates, including CI if `ci.wait` is set.
- After a wave the tool prints the results and asks `Continue with N remaining repos? [y/N]` when interactive.
- Non-interactive: it stops after the wave and exits with code 2; `--continue-if-green` continues automatically only if every repo of the wave reached a successful result. Otherwise it stops with a non-zero exit code.
- `--waves 1,5,all` gives more steps, `--waves all` skips the ramp-up.
- Because runs are idempotent, continuing simply skips `already_done` repos.

**Exit codes:** 0 all repos successful; 1 at least one repo not successful; 2 stopped after a wave waiting for a human; 3 refused before starting (validation, permissions, credentials, unchecked agent); 4 the tool itself crashed (see "Crashes of the tool").

