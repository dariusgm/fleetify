# M11a: open items of outage handling

**Status:** open, all small. The outage handling for hosts, CI, merge, push, rate limits, reads, cache fetch and clone is built. These points remain.

1. **Reset times from agent messages.** The reset time is carried as printed. Parse it into a wait, so that a `rate_limited` result carries a time the run can act on.
2. **Rate-limit phrases.** The phrases are not checked against the real CLIs, only one credits sentence was observed. Confirm the phrases for Claude Code, Copilot and opencode, and add them to the tests. Depends on M14 step 3.
3. **Push divergence test.** It needs a race between two pushes to the same branch. Write it with a fake remote that accepts the first push and rejects the second, so the test does not depend on timing.
4. **Silence ceiling.** The agent silence ceiling can end a slow local model that is quiet after a limit line. Decide: either the ceiling ignores the time after a limit line, or a limit line is treated as a wait and not as silence.

**Done when.** Each point is either implemented with a test, or closed in DECISIONS.md with the reason.
