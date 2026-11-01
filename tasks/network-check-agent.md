# Network: check-agent runs without the recipe's network

**Status:** open, small.

**Gap.** `check-agent` (`src/cli.rs`, `check_agent_command`, and `src/run.rs`, `check_agent`) starts the agent without the recipe's `[network]`. It can therefore reach hosts the recipe does not allow, and it runs the agent with a path the real run does not use. The check passes on a setup the real run would refuse.

**Open.**

1. Decide: `check-agent` takes the recipe's network when a recipe is given, or it stays a plain health check that says so in its output.
2. If it takes the network: call `set_up_network` before the check and `tear_down_network` after it, the same as `run_agent` in `src/pipeline.rs`.
3. Test: a recipe whose allowlist does not include the model host fails `check-agent` with the refusal.

**Done when.** The behaviour is decided in `DECISIONS.md`, and the test passes.
