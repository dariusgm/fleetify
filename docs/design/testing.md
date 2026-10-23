# Testing and automation

> Design of fleetify. Moved from the former PLAN.md when it was dissolved (2026-10-23). Describes the target design; open work is in `tasks/`.

| Level | What | Where | When |
|---|---|---|---|
| 1. Unit | `repos.txt` parsing, recipe parsing and validation, credential matching, command building incl. Windows quoting, guards, redaction, event serialisation, report building | CI matrix Windows + Linux | every commit |
| 2. Pipeline E2E with fakes | local bare repos as remotes, a fake host and CI HTTP server (merge requests, pipelines, jobs, logs), fake agent scripts (apply a patch, hang, ask a question, crash, write `blocked.md`, make forbidden changes, commit by itself, read or write outside the workspace to test the canary check); assertions on `report.json` and `events.jsonl` | CI matrix Windows + Linux | every commit |
| 3. E2E with a local model | opencode with a llama.cpp server; asserts pipeline invariants only, not model quality | nightly job or a workstation (needs a model download and CPU/GPU time) | nightly, before release |
| 4. Real hosts | throwaway repos on GitHub and a private project on gitlab.com (with a trivial pipeline); a test project on the self-hosted GitLab (only reachable inside the company network) | scheduled job with secrets; GitLab part inside the company by hand | weekly or manual |
| 5. Real agents | Claude Code (private), Copilot CLI (work) on test repos | manual script (`scripts/e2e-*`) | before a release (costs tokens, rate limits) |

Levels 1 and 2 are cheap to automate fully and give most of the confidence, so they come first. Level 3 is added once an agent adapter exists. Levels 4 and 5 are repeatable scripts run by hand at first. How much further to automate is decided after stage 1.

Invariants every E2E level asserts: no push when a gate failed; failed, missing or blocked CI never leads to a merge; no change outside the workspace; secrets never in logs, events or the report; timeouts leave no running processes; a re-run creates no duplicate branches or PRs; every non-successful repo has a `failure` block in the report with a non-empty message; `verify_failed`, `agent_timeout`, `needs_input` and `blocked` are valid outcomes while a PR despite a failed gate is a bug.

Company repos probably must only be processed with the agent approved there (Copilot). That needs clearing with compliance and is not a detail of the tool.

