# Bug: CI status requests fail during the wait, retry counter stuck

**Status:** open. Seen in every trial run.

**Symptom.** During the CI wait the status request fails with `cannot reach the host: error sending request for url (...check-runs...)`. The event shows `attempt: 1` for every retry, so the counter does not advance. The transport error chain is not printed, so the real cause is hidden.

**What is known.**

- `curl` and `gh` reach the same API endpoint from the same machine.
- The error happens in the HTTP client of fleetify (`src/hosts.rs`, `Http::send`).
- The retries continue for the whole wait; they do not end the repository, so the wait completes.

**Next steps.**

1. Print the full `reqwest` error chain (`source()` of each level) in the transport error. Expect the real cause (DNS, TLS, timeout, connection reset).
2. Fix the attempt counter in `retry_read` or in the CI poll, so each retry increments it.
3. Reproduce with a fake server that drops the first connection, and assert the counter and the chain.

**Done when.** A retry event shows an increasing attempt number and a cause that names the failing layer.
