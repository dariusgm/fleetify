# M14: full level-5 lint test and rate-limit behaviour

**Status:** open. The smoke test on GitHub passed (PR #1); the dependency-update test with Claude Code and Haiku passed on the icalendar fork, so the adapter works end to end. What is missing is the lint case with the complete gate chain and the documentation of the rate limit.

**Goal.** Claude Code fixes a deliberately introduced lint violation on a throwaway repository and opens a pull request that passes every gate: local verifier, guards, CI.

**Steps.**

1. Create a throwaway repository with one lint violation (for example an unused import in Python), a CI that runs the linter, and a verifier that runs it too.
2. Run the recipe with `agent = "claude-code"` and the default model. Record the run in `DECISIONS.md`.
3. Observe Claude Code's rate-limit behaviour: run a batch of repos until a limit is hit, and write down the exact message, the exit code and how fleetify classified it (`rate_limited`, `api_limit`, or other).
4. Delete the throwaway repository with the `fleetify_clean` skill.

**Done when.** The PR passes the gates, the rate-limit observation is documented with the real message, and `rate_limited` / `api_limit` are either confirmed or corrected.

**Note.** The model is passed through unchanged; use the model you want to test.
