# Availability: outages of hosts and CI

> Design of fleetify. Moved from the former PLAN.md when it was dissolved (2026-10-23). Describes the target design; open work is in `tasks/`.

Hosts and CI servers can be unreachable for minutes or hours: the company GitLab is only reachable inside the network, GitHub and gitlab.com have outages, and a scheduled run may start while a server is down. The rule is that an outage never produces a false result and never produces a merge.

1. **Transient errors are retried, not fatal.** Connection errors, timeouts, HTTP 5xx and 429 (with `Retry-After` when present) are retried with exponential back-off and jitter, inside the same budget as the CI gate (`timeout_min`). The current code ends the repo as `error` on the first failed request in `ci::poll`; that is wrong for these errors. Other 4xx answers stay immediate errors.
2. **No verdict from partial data.** A poll that could not read every item of the commit gives no verdict for that poll. Only a complete, successful read can end the gate as `ci_passed`. A failed read never counts as green, pending or missing.
3. **Outage beyond the budget: `host_unreachable`.** A new result, not successful, with a `failure` block that names the host, the operation, the last error, the last time a read succeeded, the pushed SHA and the change request URL. It is distinct from `error` (a bug or a bad answer) and from `ci_timeout` (the pipeline is still running). Exit code 1 as for other unsuccessful results.
4. **Resume is the normal path.** A re-run with the same run id reads `state.json`, sees the pushed SHA and the change request, and continues at the CI gate for that SHA (rule 3 of "Run flow per repo"). It never pushes again and never opens a second change request. The CI verdict of an earlier poll is stored in `state.json` with its time, so a re-run after a long outage shows what was known before.
5. **Merge is never retried blindly.** Before a merge the orchestrator reads the change request. If it is already `merged`, the result is `merged`, not `merge_blocked` (a merge call may have succeeded on the server while its response was lost). If the host is unreachable when the merge is sent, the repo ends as `host_unreachable` and the merge is repeated only by a re-run, which starts with the same check. The expected-head guard stays on every attempt.
6. **Other repos go on.** An outage of one host ends only the repos that use it. With `--parallel`, repos on other hosts keep running. The run does not abort, and the report lists the affected repos with `host_unreachable`.
7. **Git operations get the same treatment.** `ls-remote`, `fetch` and `clone` are retried like API calls. A `push` that timed out is checked before it is repeated: if the remote branch already points at the pushed SHA, the push is done; if it points elsewhere, the repo ends as `error` (a branch with foreign commits is never overwritten).
8. **Automated callers can act on it.** The report carries `host_unreachable` with the host and the time of the last successful read, so a scheduler can rerun later. The exit code does not change: 1 means "not successful", and the report says why.
9. **Visible in the event stream.** Every retry is an event (`host_retry` with host, operation, attempt, delay, error class), so a live view or a log can show a server being down without treating it as a failure.

**Tests** (against the fake host and fake CI server): a read that fails twice and then succeeds gives the normal verdict; a 429 with `Retry-After` is honoured; an outage longer than `timeout_min` gives `host_unreachable` with the failure block; a re-run after recovery continues at the gate without a second push or change request; a merge response lost after the server merged gives `merged` on the re-run; a push whose remote branch points elsewhere is refused, not overwritten; a repo on an unreachable host does not stop a repo on a reachable host.

**Why this is stage 1, not stage 2:** the company GitLab is only reachable inside the company network, and scheduled runs will hit outages. Without rules 1–5 a single network blip gives a wrong result and a retry gives a duplicate or a missed merge.

### Rate limits (extension of M11a)

Rate limits come from two sides, and each needs its own handling. Neither is implemented yet.

**Hosts and CI (fleetify's own requests).**

1. **Use the server's answer.** `Retry-After` (seconds or HTTP date) sets the wait when present. The rate-limit headers of the host set it otherwise: when the remaining quota is used up, wait until the reset time instead of guessing with back-off. Header names are taken from the current API docs of each host and noted in the host module (GitHub: `x-ratelimit-remaining`, `x-ratelimit-reset`; GitLab: `RateLimit-*`; to be verified).
2. **Secondary limits count too.** GitHub reports some limits as HTTP 403 with its own message. Those are rate limits, not access errors, and must not end as `Forbidden`.
3. **Shared per host.** Parallel repos on one host share one limiter. When one repo has to wait, the others on that host wait too instead of hitting the same limit again.
4. **Configured ceiling.** The recipe's `[rate_limit] max_wait_min = 15` is an absolute maximum for one wait. A wait longer than that is not taken; the repo ends as `rate_limited`. Without the key a default ceiling applies, so the run never waits without bound. An optional proactive limit (`max_requests_per_min` per host, off by default) can be added later.
5. **Own result.** `rate_limited` is a new result, not successful. It is distinct from `host_unreachable` (the server did not answer) because the server did answer and told us when to come back. Its failure block names the host and the reset time. A re-run after the reset continues, as for outages.
6. **Visible.** Each wait is a `rate_limited` event with host, wait time and reason, so a live view or a log shows it without treating it as a failure.

**Agents (the coding agent's own rate limits).** The agent's connection to its model provider is not fleetify's connection. Claude Code waits and blocks on its own; opencode and Copilot CLI behave differently and are not known yet. Today the only protection is the idle warning and `agent_timeout`, which ends the repo as a generic timeout and hides the cause. Planned:

7. **Classify by adapter.** `Agent::classify` recognises the adapter's rate-limit message in the output and returns a rate-limit outcome. The repo ends as `rate_limited` with the message in the failure block, not as `agent_timeout`.
8. **Ceiling for agents too.** The same `max_wait_min` applies if the agent is known to wait for its limit. An agent that waits longer than the ceiling is stopped and the repo ends as `rate_limited`; a re-run continues.
9. **What can be tested.** Classification and the result mapping are tested with fake agents that print a rate-limit message and exit (no real limit needed). The real behaviour of each CLI is observed only in the level-5 runs (M14 for Claude Code) and documented there.

**Tests** (fake host and fake agent): a `Retry-After` value is honoured; a reset header is used instead of back-off; a wait above `max_wait_min` ends as `rate_limited` with the reset time; a 403 with the secondary-limit message is a rate limit; two repos on one host wait on the same limiter; a fake agent printing a rate-limit message ends as `rate_limited`, not `agent_timeout`; a re-run after the reset continues.

**Acceptance** is added to M11a: every test above passes on both OS, and the M14 level-5 run documents Claude Code's observed behaviour when it hits its own limit.

