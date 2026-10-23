# Host layer (GitHub and GitLab)

> Design of fleetify. Moved from the former PLAN.md when it was dissolved (2026-10-23). Describes the target design; open work is in `tasks/`.

Everything host-specific sits behind a small trait, one module per host (`hosts/github.rs`, `hosts/gitlab.rs`):

```rust
pub struct ChangeRequest { pub url: String, pub id: String /* PR number / MR iid */, pub head_sha: String, pub state: CrState /* Open | Merged | Closed */ }

pub trait Host {
    fn probe_read_access(&self, repo: &Repo) -> Result<Access>;           // Ok | Unauthorized | Forbidden | NotFound
    fn default_branch(&self, repo: &Repo) -> Result<String>;
    fn find_change_request(&self, repo: &Repo, branch: &str) -> Result<Option<ChangeRequest>>;   // open PR/MR for the branch
    fn create_change_request(&self, repo: &Repo, branch: &str, base: &str, title: &str, body: &str) -> Result<ChangeRequest>;
    fn merge(&self, repo: &Repo, cr: &ChangeRequest, expected_sha: &str) -> Result<MergeOutcome>;  // Merged | Blocked(reason)
}
```

Implemented via the REST APIs (no `gh`/`glab` dependency). Paths and parameters must be checked against the API docs of the deployed version. GitLab project paths with subgroups are URL-encoded (`/` becomes `%2F`), API base `https://<host>/api/v4`. Whether a remote branch exists is checked with `git ls-remote` by the orchestrator, not via the API.

CI is deliberately not part of the `Host` trait: the CI system can differ from the host (for example GitLab with an external CI). See "CI layer and CI gate".

