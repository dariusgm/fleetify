# Agents: which one to pick

fleetify runs one coding agent per recipe, on a copy of the repository, and checks its change with the verifier, the guards and, optionally, a judge. This folder describes the three agents fleetify integrates, for the choice of agent for a recipe. The facts are from the e2e runs of 2026-11-06 and from the canary checks; each file says what was tested and what was not.

| Agent | Model | Where it runs | Cost | Inner loop (verifier) | Best for |
|---|---|---|---|---|---|
| [Claude Code](claude-code.md) | `haiku` (pinned) | host | Anthropic account | works with the granted verifier | the most predictable runs we have |
| [Copilot CLI](copilot.md) | `auto` (not choosable) | host | GitHub Copilot plan | works, with a broader grant (`sh`) | when an existing Copilot plan is the budget |
| [opencode](opencode.md) | `llamacpp/qwen3.8-flash-next` (local) | host | local hardware | not re-run since the fix | no external model and no account |

Common to all three:
- The canary (`fleetify check-agent <name>`) must pass before a run, unless the recipe is run with `--allow-unchecked-agent`. A canary can fail once and pass on the next try with the same flags, so rerun it before you judge an agent broken.
- A run's change is checked by the same verifier twice: by the agent in its loop, then by fleetify on the trusted copy. Nothing is pushed unless both pass.
- The judge (Jev, see `tasks/m16-jev-prompt/README.md`) is independent of the agent.
- Every e2e case uses an explicit base branch: readme and deps from `main`, the gaming case from `m16/base-broken`.
