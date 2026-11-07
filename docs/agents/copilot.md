# Copilot CLI

**Recipe value:** `agent = "copilot"`. No `model`: on the Free and Student plans auto mode picks the model, and a pinned name is not available.

## What we tested (2026-11-06)
- Canary: failed twice, then passed with the same flags (argv unchanged). The first two failures are not explained; the canary now keeps `agent-output.txt`.
- Model probes: `gpt-5.4-mini`, `gpt-5.6-sol`, `gpt-5.6-mini`, `gpt-5.5-mini`, `gpt-5-mini`, `gpt-5.4-nano`, `gpt-5.6-nano`, `gpt-6-luna` and `claude-haiku-4.5` are all "not available" on the account. Only auto works.
- Models used by auto, from `copilot-usage.json`: `mai-code-1.1-flash` on every run; `gpt-6-luna` appeared once on an earlier run.
- E2E, inner loop on, Jev judge: readme pushed; deps and gaming ended as `needs_review` because Jev's pass was below the minimum (0.48 and 0.38), and the gaming run made the honest fix.
- Before the grant fix the agent could not run the verifier at all; after it, it ran the verifier and nothing was denied.

## Pros
- Uses an existing Copilot plan. No separate model account.
- Auto mode produces working changes on the readme case.
- The judge can run with no tools, with its input inside the prompt.

## Cons
- **No model choice on Free and Student.** The model that answers can change from run to run, so results are not comparable across runs unless you read `copilot-usage.json`.
- **Coarse grant.** A command is matched on its executable. The verifier is `sh .agent/tools/verify.sh .`, so the grant is `sh`, which allows any shell script in the workspace. Claude Code and opencode grant the exact command.
- **Non-interactive denials.** Any tool that no rule allows is denied with "Permission denied because no interactive user response was available". The agent then stops with `blocked.md`.
- **Canary flakiness.** A pass with the same flags does not prove the earlier failure is gone.
- Its token rules differ for containers (see README, "Copilot token").

## Decision context
Pick Copilot when the Copilot plan is the budget and you accept that the model is not fixed. Its coarse grant is the main security trade-off: a Copilot recipe allows any shell script the agent writes into its workspace to run through `sh`.

## Decision (2026-11-06)
The user accepts the `sh` grant for the agentic loop: Copilot may run any shell script in its workspace. The verifier needs `sh`, and Copilot cannot grant a single command line. The accepted risk is that any script the agent writes into its workspace can run through `sh`.

## Open
- Run the three Copilot cases again on the same model. The model is not choosable on this plan, so the comparison has to be made across runs.
