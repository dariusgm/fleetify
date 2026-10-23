# CI layer and CI gate

> Design of fleetify. Moved from the former PLAN.md when it was dissolved (2026-10-23). Describes the target design; open work is in `tasks/`.

**Purpose.** The CI gate is the last check before any merge: the repository's own CI must pass on the exact commit fleetify pushed. It is separate from the verifier. The verifier is fast, local and runs inside the agent loop. CI runs the integration tests and everything slow, in the company's own environment.

**CI is separate from the host.** The CI system can differ from the Git host, so CI has its own trait, one module per CI system, reusing the host's API client and credential:

```rust
pub trait CiProvider {
    fn name(&self) -> &str;
    fn pipelines(&self, repo: &Repo, sha: &str, cr: Option<&ChangeRequest>) -> Result<Vec<Pipeline>>;   // everything CI knows about this commit
    fn state(&self, pipelines: &[Pipeline]) -> CiState;                     // None | Pending | Success | Failed | Blocked
    fn failure_details(&self, repo: &Repo, pipelines: &[Pipeline], max_lines: usize) -> Result<Vec<FailedJob>>;   // name, stage, url, log tail
}
```

**Stage 1 supports GitLab CI only** (`ci/gitlab_ci.rs`). `provider = "auto"` selects it for GitLab repos. For a GitHub repo, `wait = true` fails validation before anything runs ("no CI provider for this host yet, set wait = false or contribute one"); GitHub flows end at `pr_open`. Other CI systems (GitHub Actions, Jenkins) are one module each and can come later or from the community.

### The gate, independent of the provider

Settings (`[ci]` in the recipe): `provider` (default `"auto"`), `wait` (default `false`; without it there is no CI check and the run ends at `pr_open`), `appear_timeout_min` (default 5), `timeout_min` (default 30, counted from the push), `poll_interval_s` (default 30, with jitter and back-off on rate limits), `failure_log_lines` (default 40), `merge` (`"off"` | `"when_green"`).

1. After the push and the PR/MR creation, poll until the state is final or a timeout hits. Polling is a separate task: the agent slot is free once the push is done, and waiting time is not agent time.
2. **No pipeline within `appear_timeout_min`: `ci_missing`.** No CI never counts as green. Typical causes: no pipeline configuration, rules that exclude the branch or merge requests, skip markers, a blocked pipeline creation. fleetify never adds skip-CI markers to commit messages, and recipe validation rejects a commit message or PR title containing one.
3. **Aggregation:** consider every pipeline of the exact pushed commit (branch pipelines and merge request pipelines). The gate is green only if at least one exists, none is still pending or running, and all of them succeeded. Any failed or canceled pipeline is `ci_failed`. A pipeline waiting for a manual job is `ci_blocked` (needs a human; fleetify never triggers manual jobs). Skipped pipelines do not count as success; if only skipped ones exist the result is `ci_missing`.
4. Overall timeout: `ci_timeout` (the pipeline may still be running; the report carries the link).
5. **The result belongs to the pushed commit.** Before a merge the head of the MR must still be that commit (see below).
6. No re-triggers and no retries of jobs in stage 1, not even for flaky jobs.
7. **Failure details:** on `ci_failed` the failed jobs (name, stage, URL) are recorded as events and in the report, and optionally the last `failure_log_lines` lines of each failed job's log. That text is untrusted: redacted, length-limited, displayed only, never executed and not handed to the agent in stage 1 (the CI-fix loop in stage 2 will).
8. Green CI without a merge ends as `ci_passed`.

### GitLab CI provider (first implementation)

- Uses the REST API: list the project's pipelines for the commit SHA, read their status and jobs, and read job logs for failure details. Endpoints and parameters must be checked against the API documentation of the deployed GitLab version; they have not been verified here.
- Also include the merge request's own head pipeline, in union with the pipelines found by SHA.
- Status mapping (to be verified against the docs): `created`, `waiting_for_resource`, `preparing`, `pending`, `scheduled`, `running` map to pending; `success` to success; `failed`, `canceled` to failed; `skipped` to skipped; `manual` to blocked. An unknown status is treated as pending, logged as a warning, and ends as `ci_timeout` if it never resolves.
- The credential needs read access to pipelines and jobs; merging needs more. Check the exact token scopes in the docs and write them into the README's credentials section.
- **Not covered in the first version:** results that arrive only as external commit statuses (for example from Jenkins), merge trains and merged-results pipelines, per-job "required" lists, and special handling of `allow_failure` jobs (the pipeline's own aggregate status is used as is). Downstream pipelines count only as far as the pipeline status already includes them.

### Merge after CI

Merging stays a two-key operation (`merge = "when_green"` and `--merge`). In addition: immediately before merging, the orchestrator re-reads the MR head and requires it to equal the pushed commit and the gate to be green for that commit. The merge call passes that commit as the expected head (the GitHub and GitLab merge APIs support such a guard; verify in the docs). A different head means `merge_blocked` with the reason in the event log and report. The host's branch protection, approvals and required-pipeline rules apply as usual, and any refusal is `merge_blocked`; fleetify never forces a merge.

### Tests for the CI layer

Against a fake GitLab API: success; failed; canceled; no pipeline ever; only skipped; manual-blocked; slow pipeline until timeout; pipeline appearing late; branch pipeline and MR pipeline with one failing; rate limit (429) while polling; head commit changed before merge; unknown status. Plus one real run against a private gitlab.com project with a trivial pipeline, and later one against the self-hosted GitLab at work.

