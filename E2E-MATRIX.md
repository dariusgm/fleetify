# E2E test matrix

Every combination that has been run for real, plus the combinations that are still open. A row is
`done` only when a real run exists (run id in the last column). Fake-host and fake-agent tests are in
`tests/` and listed separately; they show the logic, not the real CLI or host behaviour.

Dimensions: host (GitHub, GitLab), recipe kind (no agent, agent), agent (claude-code, copilot,
opencode), model (agent default, OpenAI `gpt-5-nano`, llama.cpp `qwen3.8-flash-next`), CI wait,
merge, run mode (dry run, real run).

The model is an agent dimension, not a fleetify one: the generated grants and the gates are the same
for every model. Models still differ in how they call tools (see the opencode finding below), so both
opencode models are kept as rows.

## Real runs

| # | Host | Recipe | Agent | Model | CI wait | Merge | Mode | Result | Evidence |
|---|------|--------|-------|-------|---------|-------|------|--------|----------|
| 1 | GitHub | cargo-deps-update | none | — | off | off | real | pr_open | fleetify PR #1 (run `cargo-deps-update-remote`) |
| 2 | GitHub | cargo-deps-update-csvtojson | none | — | off | off | real | pr_open | CsvToJson PR #3 (run `csvtojson-1`) |
| 3 | GitHub | poetry-to-uv | none | — | off | off | real | pr_open, then merged | blog PR #14, #15; `poetry-to-uv-merge` |
| 4 | GitHub | smoke-test | none | — | off | off | real, re-run | already_done | blog PR #13; run `smoke-2` (no second PR) |
| 5 | GitHub | cargo-deps-update | none | — | off | off | real | verify_failed | git2duck (baseline) |
| 6 | GitLab | ruby-34-update | none | — | off | off | real | pr_open | ruby-hello MR !1 |
| 7 | GitLab | ruby-34-update | none | — | off | when_green, no policy | real | merge_blocked | `ruby34-merge` |
| 8 | GitLab | ruby-34-update | none | — | off | when_green + policy | real | merged | `ruby34-merge2` |
| 9 | GitLab | ruby-34-copilot | copilot | default | off | off | real | needs_input | `copilot-ruby` (grant denied) |
| 10 | GitLab | ruby-34-copilot | copilot | default | off | off | real | pr_open | `copilot-ruby2`, MR !1 |
| 11 | GitHub | cargo-deps-update | none | — | off | off | dry run | dry_run | `skill-check-dry` (four repos) |
| 12 | GitHub | ruby-34-opencode | opencode | OpenAI gpt-5-nano | off | off | real | pr_open | throwaway PR #1; agent ran the verifier through `bash -lc` and was denied (see finding A) |
| 13 | GitHub | ruby-34-opencode | opencode | llama.cpp qwen3.8-flash-next | off | off | real | pr_open | throwaway PR #2; agent ran the verifier directly, passed |
| 14 | GitHub | ruby-34-opencode | opencode | OpenAI gpt-5-nano | off | off | dry run | dry_run | `m10-smoke-dry` |
| 15 | GitHub | any | any | any | on | off | real | pr_ci_green | reported by the owner as tested; no run record on this machine |

Rows 1–10 used repos that were deleted (2026-10-21) except csvtojson, blog, git2duck, cargo-deps and
fleetify. Rows 7–10 can no longer be re-run without new repos. Rows 12–13 use
`dariusgm/fleetify-m10-smoke` (still present, PRs #1 and #2 open).

## Canary checks (`check-agent`)

| Agent | Model | Result |
|-------|-------|--------|
| claude-code | default | passed (repeated, M14) |
| copilot | default | passed (M15a) |
| opencode | OpenAI gpt-5-nano | passed |
| opencode | llama.cpp qwen3.8-flash-next | passed |

## Failure outcomes

| Outcome | Real run | Fake tests only |
|---------|----------|-----------------|
| verify_failed | yes (5) | — |
| needs_input (agent asked, or grant denied) | yes (9) | yes |
| merge_blocked | yes (7) | yes |
| pr_open → already_done on re-run | yes (4) | yes |
| api_limit (missing usage credits) | the CLI message only (`--model fable`, no run) | yes |
| rate_limited (agent) | no | yes (print + silence) |
| agent_timeout | no | yes |
| guard_violation | no | yes |
| ci_failed / ci_blocked / ci_timeout / ci_missing | no | yes (GitLab, GitHub) |
| host_unreachable | no | yes |

## Open combinations (not run for real)

- **GitHub with merge when_green**, real: no run yet.
- **GitLab with an agent other than Copilot**: claude-code and opencode on GitLab never ran.
- **Copilot on GitHub**: never run for real.
- **Agent rate limit for real**: deliberately not provoked. A real call into a limit costs money, so it
  waits until a limit is hit by chance. The phrases are a starting list.
- **Windows real runs**: only the CI matrix builds and tests on Windows; no real pipeline run there.
  PowerShell on Linux (7.6.7) runs the `verify.ps1` scripts, which is parity for the scripts, not for
  Windows itself (paths, CRLF, Windows PowerShell 5.1, process spawning).
- **Re-run after `api_limit` or `rate_limited`**: the resume path after these new results is untested.
- **`--model` per agent for claude-code and copilot on the real CLIs**: adapters are unit-tested only.
- **Local llama.cpp with a larger repo or a longer task**: the 3.2→3.4 task is small; time per token is
  long on this server, so agent timeouts with the local model are not exercised.
- **Push race on an existing branch** (plan's push-divergence case): needs a deliberate race.

## Findings

- **A. The wrapped verifier call (opencode with OpenAI).** Fix in PR #25 (rules v2: the verifier command
  must run as its own command). Not yet re-run. The agent ran `bash -lc 'sh .agent/tools/verify.sh .'`
  and was denied by the grant. A direct test shows the plain form `sh .agent/tools/verify.sh .` passes under
  the generated grant `sh .agent/tools/verify.sh *`. So the grant is correct and the wrapper came from the
  model. The llama.cpp model ran the plain form. Options: state the exact command in the task file (no
  wrapper), or accept it. Widening to `bash *` is not an option.
- **B. The agent's own verification is not authoritative.** The gate runs the verifier after the agent
  (both runs). An agent's `result.md` that claims a pass is not evidence.
- **C. Test repos must be recreated for rows 7–10.** Deleting them removed the only real evidence for
  GitLab merges and Copilot.
