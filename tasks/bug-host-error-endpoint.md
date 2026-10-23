# Bug: host errors do not name the endpoint

**Status:** open. Seen in the VoiceStudio reference run: `error` with the message `HTTP 404: Not Found` and no hint which call failed.

**Goal.** Every host error says which request failed: the method and the path (without the token), for example `POST /repos/o/r/actions/workflows/ci.yml/dispatches: HTTP 404 Not Found`.

**Where.** `unexpected()` and `refusal_reason()` in `src/hosts.rs`, and the CI dispatch and CI read calls in `src/hosts/github.rs` and `src/hosts/gitlab.rs`.

**Done when.**

- Every `HostError` from a request carries the method and the path.
- Tokens never appear (test with a token in the query string).
- A test asserts the endpoint in the message for one failing call per provider.
