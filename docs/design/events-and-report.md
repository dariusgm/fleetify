# Events, renderers and the report

> Design of fleetify. Moved from the former PLAN.md when it was dissolved (2026-10-23). Describes the target design; open work is in `tasks/`.

**One event stream.** The orchestrator never prints. It emits typed events (`serde`-serialisable enum) into a broadcast channel: `run_started` (recipe, run id, effective permissions, config hash), `repo_phase` (repo, phase), `command_started` / `command_finished` (argv redacted, exit code, duration), `output_line` (repo, stream, line), `verifier_result`, `guard_violation`, `judge_verdict`, `agent_idle`, `ci_poll`, `ci_result`, `repo_finished` (repo, result, details), `wave_finished`, `run_finished`. Consumers subscribe to the channel; the core does not know who is listening.

**Consumers in stage 1:**

- **Event log writer:** `events.jsonl` per repo, append-only, one JSON object per event. Always on.
- **State writer:** `state.json` per repo (current phase, start time, last output time, last N output lines, result, pushed SHA, PR/MR URL, CI result), written atomically (temp file and rename) so another terminal can read it at any time. Always on.
- **Log file writers:** `agent.log`, `verify-<n>.log`, `prepare.log`, `task.md` (as rendered), `diff.patch`, `result.md` / `blocked.md` if present. Always on.
- **Report writer:** builds `report.json` from the final events; see below. Always on.
- **Plain renderer:** prefixed lines like `docker compose logs` (`repo-name | line`), phase changes, a periodic summary line (e.g. every 60 s: `5 running, 2 done, 1 failed`), and a final table. Default whenever stdout is not a TTY or `--plain` is given. Everything goes to stdout; the tool's own diagnostics go to stderr.
- **TUI renderer (optional, `--ui`):** see "Live view".

Nothing a renderer shows comes from anywhere but the event stream, so the live view can never contain information that is missing from the logs and the report.

**Report (`report.json`).** The primary output for automated use, written at the end of the run and rewritten on every re-run of the same run id. Structure:

```json
{
  "schema": 1, "run_id": "...", "recipe": "...", "started": "...", "finished": "...", "exit_code": 1,
  "summary": {"total": 12, "successful": 10, "by_result": {"pr_open": 9, "skipped": 1, "verify_failed": 2}},
  "repos": [
    {
      "repo": "gitlab.com/group/sub/project-c", "result": "verify_failed", "successful": false,
      "phases": {"cloning": 3.1, "agent": 412.0, "verifying": 18.4},
      "change_request": null, "pushed_sha": null,
      "failure": {
        "class": "verify_failed",
        "message": "verifier exited with 1 (trusted copy)",
        "rule": null,
        "output_tail": ["...last 40 lines of verify-2.log..."],
        "agent_result_md": "...length-limited...", "agent_blocked_md": null,
        "artifacts": {"events": "…/events.jsonl", "agent_log": "…/agent.log", "verify_log": "…/verify-2.log", "diff": "…/diff.patch", "workspace": "…"}
      }
    }
  ]
}
```

`failure` is present for every non-successful repo and carries the one thing a caller needs to decide what to do next: for `guard_violation` the rule and offending lines, for `ci_failed` the failed jobs with URLs and log tails, for `needs_input` / `blocked` the prompt or `blocked.md`, for `agent_timeout` the last output lines, for `error` the error chain. All text in it is redacted and length-limited. `fleetify run --format json` prints the report to stdout instead of the table; `--report PATH` writes a copy elsewhere. `fleetify report [--run NAME] [--format table|json] [--failed]` renders the same from the run folder later.

**Crashes of the tool.** `tracing` with a JSON subscriber writes `fleetify.log` into the run folder (level `info` by default, `--debug` for `debug`, `RUST_LOG` honoured). A panic hook writes the panic message and backtrace to `fleetify.log` and stderr, terminates all child process trees, marks running repos `error` with the panic message as failure, still writes `report.json`, and exits with code 4. `--debug` additionally keeps every subprocess's full output in the events.

**`fleetify status [--run NAME] [--watch]`** shows a table from the state files (repo, phase or result, elapsed, idle time, last line, PR/MR link), usable from a second terminal while a run is active. Default run: the most recently modified. **`fleetify logs <repo> [--run NAME] [-f] [-n 50]`** tails one repo's agent log.

**Ctrl-C:** terminates all child process trees, marks running repos `interrupted`, keeps workspaces, writes the report, prints the summary. A re-run continues according to the resume rules.

