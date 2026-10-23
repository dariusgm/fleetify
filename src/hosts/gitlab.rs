//! GitLab REST API (v4). Docs: <https://docs.gitlab.com/api/rest/> (checked 2026-10).
//! Endpoints used: `GET /projects/:id`, `GET|POST /projects/:id/merge_requests`,
//! `PUT /projects/:id/merge_requests/:iid/merge`, `GET /projects/:id/pipelines?sha=`.
//! `:id` is the URL-encoded full path.

use reqwest::Method;
use serde_json::{Value, json};

use super::{
    Access, ChangeRequest, CiFailure, CiItem, CiState, CrState, Host, HostError, Http, LogBudget,
    MAX_CI_FAILURES, MergeOutcome, Reply, access_from_status, log_tail, parse_json, refusal_reason,
    unexpected,
};
use crate::credentials::Secret;
use crate::events::EventBus;
use crate::repos::Repo;

pub struct GitLab {
    http: Http,
}

impl GitLab {
    pub fn new(http: Http) -> Self {
        GitLab { http }
    }
}

/// The project id for a full path: `group/sub/name` becomes `group%2Fsub%2Fname`.
/// Paths only contain `[A-Za-z0-9._-]` and `/` (see repos.rs), so this is exact.
pub fn project_id(path: &str) -> String {
    path.replace('/', "%2F")
}

fn headers(token: &Secret) -> Vec<(&'static str, String)> {
    vec![("PRIVATE-TOKEN", token.expose().to_string())]
}

fn change_request(value: &Value) -> Result<ChangeRequest, HostError> {
    let iid = value["iid"]
        .as_u64()
        .ok_or_else(|| HostError::Parse("merge request has no iid".into()))?;
    let state = match value["state"].as_str() {
        Some("opened") => CrState::Open,
        Some("merged") => CrState::Merged,
        Some("closed") | Some("locked") => CrState::Closed,
        other => {
            return Err(HostError::Parse(format!(
                "unknown merge request state {other:?}"
            )));
        }
    };
    Ok(ChangeRequest {
        url: value["web_url"].as_str().unwrap_or_default().to_string(),
        id: iid.to_string(),
        head_sha: value["sha"].as_str().unwrap_or_default().to_string(),
        state,
    })
}

/// Maps a pipeline status to a CI item. `None` means the pipeline is left out.
fn pipeline_state(status: &str) -> Option<CiState> {
    match status {
        "success" => Some(CiState::Success),
        "failed" | "canceled" => Some(CiState::Failure),
        "skipped" => None,
        // `manual` waits for a person: the gate ends as `ci_blocked`, never as a merge.
        "manual" => Some(CiState::Blocked),
        // Unknown statuses are treated as pending, the same as running ones.
        _ => Some(CiState::Pending),
    }
}

/// Child pipelines nest at most this deep; a deeper tree is cut, not followed.
const MAX_PIPELINE_DEPTH: u32 = 5;

impl GitLab {
    /// The failed jobs of one pipeline, then those of the child pipelines it triggered.
    /// Trigger jobs are skipped: their own log says nothing about the cause.
    fn collect_failures(
        &self,
        project: &str,
        token: &Secret,
        pipeline: u64,
        budget: LogBudget,
        depth: u32,
        out: &mut Vec<CiFailure>,
    ) -> Result<(), HostError> {
        if depth > MAX_PIPELINE_DEPTH || out.len() >= MAX_CI_FAILURES {
            return Ok(());
        }
        let bridges = self.list(
            &format!("/projects/{project}/pipelines/{pipeline}/bridges"),
            token,
        )?;
        let trigger_ids: Vec<u64> = bridges.iter().filter_map(|b| b["id"].as_u64()).collect();
        let mut jobs = self.list(
            &format!("/projects/{project}/pipelines/{pipeline}/jobs"),
            token,
        )?;
        // The earliest start is the likeliest cause; jobs that never started come last.
        jobs.sort_by(|a, b| {
            (a["started_at"].is_null(), a["started_at"].as_str())
                .cmp(&(b["started_at"].is_null(), b["started_at"].as_str()))
        });
        for job in &jobs {
            let job_id = job["id"].as_u64().unwrap_or_default();
            let failed = job["status"].as_str() == Some("failed")
                && job["allow_failure"].as_bool() != Some(true);
            if !failed || trigger_ids.contains(&job_id) {
                continue;
            }
            let reply = self.http.send(
                Method::GET,
                &format!("/projects/{project}/jobs/{job_id}/trace"),
                &headers(token),
                &[],
                None,
            )?;
            let log_tail = if reply.status == 200 {
                log_tail(&reply.body, budget)
            } else {
                vec![format!("log not available: HTTP {}", reply.status)]
            };
            out.push(CiFailure {
                name: job["name"].as_str().unwrap_or("job").to_string(),
                log_tail,
            });
            if out.len() >= MAX_CI_FAILURES {
                return Ok(());
            }
        }
        for bridge in &bridges {
            if let Some(child) = bridge["downstream_pipeline"]["id"].as_u64() {
                self.collect_failures(project, token, child, budget, depth + 1, out)?;
            }
        }
        Ok(())
    }

    /// One GET that answers with a JSON array.
    fn list(&self, path: &str, token: &Secret) -> Result<Vec<Value>, HostError> {
        let reply = self.http.send(
            Method::GET,
            path,
            &headers(token),
            &[("per_page", "100")],
            None,
        )?;
        if reply.status != 200 {
            return Err(unexpected(&reply));
        }
        parse_json(&reply)?
            .as_array()
            .cloned()
            .ok_or_else(|| HostError::Parse(format!("{path} is not an array")))
    }
}

impl Host for GitLab {
    fn attach_events(&self, bus: &EventBus) {
        self.http.attach_events(bus);
    }

    fn probe_read_access(&self, repo: &Repo, token: &Secret) -> Result<Access, HostError> {
        let reply = self.http.send(
            Method::GET,
            &format!("/projects/{}", project_id(&repo.path)),
            &headers(token),
            &[],
            None,
        )?;
        access_from_status(reply.status).ok_or_else(|| unexpected(&reply))
    }

    fn default_branch(&self, repo: &Repo, token: &Secret) -> Result<String, HostError> {
        let reply = self.http.send(
            Method::GET,
            &format!("/projects/{}", project_id(&repo.path)),
            &headers(token),
            &[],
            None,
        )?;
        if reply.status != 200 {
            return Err(unexpected(&reply));
        }
        parse_json(&reply)?["default_branch"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| HostError::Parse("project has no default_branch".into()))
    }

    fn latest_change_request(
        &self,
        repo: &Repo,
        token: &Secret,
        branch: &str,
    ) -> Result<Option<ChangeRequest>, HostError> {
        let reply = self.http.send(
            Method::GET,
            &format!("/projects/{}/merge_requests", project_id(&repo.path)),
            &headers(token),
            &[
                ("state", "all"),
                ("source_branch", branch),
                ("order_by", "created_at"),
                ("sort", "desc"),
                ("per_page", "1"),
            ],
            None,
        )?;
        if reply.status != 200 {
            return Err(unexpected(&reply));
        }
        let list = parse_json(&reply)?;
        list.as_array()
            .ok_or_else(|| HostError::Parse("merge request list is not an array".into()))?
            .first()
            .map(change_request)
            .transpose()
    }

    fn find_change_request(
        &self,
        repo: &Repo,
        token: &Secret,
        branch: &str,
    ) -> Result<Option<ChangeRequest>, HostError> {
        let reply = self.http.send(
            Method::GET,
            &format!("/projects/{}/merge_requests", project_id(&repo.path)),
            &headers(token),
            &[("state", "opened"), ("source_branch", branch)],
            None,
        )?;
        if reply.status != 200 {
            return Err(unexpected(&reply));
        }
        let list = parse_json(&reply)?;
        let items = list
            .as_array()
            .ok_or_else(|| HostError::Parse("merge request list is not an array".into()))?;
        for item in items {
            let change = change_request(item)?;
            if change.state == CrState::Open {
                return Ok(Some(change));
            }
        }
        Ok(None)
    }

    fn create_change_request(
        &self,
        repo: &Repo,
        token: &Secret,
        branch: &str,
        base: &str,
        title: &str,
        body: &str,
    ) -> Result<ChangeRequest, HostError> {
        let payload = json!({
            "source_branch": branch,
            "target_branch": base,
            "title": title,
            "description": body,
        });
        let reply = self.http.send(
            Method::POST,
            &format!("/projects/{}/merge_requests", project_id(&repo.path)),
            &headers(token),
            &[],
            Some(&payload),
        )?;
        if reply.status != 201 {
            return Err(unexpected(&reply));
        }
        change_request(&parse_json(&reply)?)
    }

    fn dispatch_ci(
        &self,
        repo: &Repo,
        token: &Secret,
        branch: &str,
        _workflow: Option<&str>,
    ) -> Result<(), HostError> {
        let reply = self.http.send(
            Method::POST,
            &format!("/projects/{}/pipeline", project_id(&repo.path)),
            &headers(token),
            &[],
            Some(&json!({ "ref": branch })),
        )?;
        match reply.status {
            201 => Ok(()),
            _ => Err(unexpected(&reply)),
        }
    }

    fn ci_failures(
        &self,
        repo: &Repo,
        token: &Secret,
        sha: &str,
        budget: LogBudget,
    ) -> Result<Vec<CiFailure>, HostError> {
        let project = project_id(&repo.path);
        let reply = self.http.send(
            Method::GET,
            &format!("/projects/{project}/pipelines"),
            &headers(token),
            &[("sha", sha), ("per_page", "100")],
            None,
        )?;
        if reply.status != 200 {
            return Err(unexpected(&reply));
        }
        let pipelines = parse_json(&reply)?;
        let mut failures = Vec::new();
        for pipeline in pipelines.as_array().into_iter().flatten() {
            let Some(id) = pipeline["id"].as_u64() else {
                continue;
            };
            self.collect_failures(&project, token, id, budget, 0, &mut failures)?;
        }
        failures.truncate(MAX_CI_FAILURES);
        Ok(failures)
    }

    fn commit_ci(&self, repo: &Repo, token: &Secret, sha: &str) -> Result<Vec<CiItem>, HostError> {
        // Every pipeline of this commit: branch pipelines and merge request pipelines alike.
        let reply = self.http.send(
            Method::GET,
            &format!("/projects/{}/pipelines", project_id(&repo.path)),
            &headers(token),
            &[("sha", sha), ("per_page", "100")],
            None,
        )?;
        if reply.status != 200 {
            return Err(unexpected(&reply));
        }
        let mut items = Vec::new();
        for pipeline in parse_json(&reply)?
            .as_array()
            .ok_or_else(|| HostError::Parse("pipeline list is not an array".into()))?
        {
            if pipeline["sha"].as_str() != Some(sha) {
                return Err(HostError::Parse(format!(
                    "asked for commit {sha}, a pipeline names {:?}",
                    pipeline["sha"].as_str()
                )));
            }
            // Skipped pipelines are left out: they never count as success, and if only
            // skipped ones exist the commit has no items and the gate reports ci_missing.
            let Some(state) = pipeline_state(pipeline["status"].as_str().unwrap_or_default())
            else {
                continue;
            };
            items.push(CiItem {
                name: format!(
                    "pipeline {} ({})",
                    pipeline["id"],
                    pipeline["ref"].as_str().unwrap_or_default()
                ),
                state,
            });
        }
        Ok(items)
    }

    fn delete_branch(&self, repo: &Repo, token: &Secret, branch: &str) -> Result<(), HostError> {
        // Branch names may contain '/', which must be encoded like a project path.
        let reply: Reply = self.http.send(
            Method::DELETE,
            &format!(
                "/projects/{}/repository/branches/{}",
                project_id(&repo.path),
                project_id(branch)
            ),
            &headers(token),
            &[],
            None,
        )?;
        match reply.status {
            204 | 404 => Ok(()),
            _ => Err(unexpected(&reply)),
        }
    }

    fn merge(
        &self,
        repo: &Repo,
        token: &Secret,
        change: &ChangeRequest,
        expected_sha: &str,
    ) -> Result<MergeOutcome, HostError> {
        let payload = json!({ "sha": expected_sha });
        let reply: Reply = self.http.send(
            Method::PUT,
            &format!(
                "/projects/{}/merge_requests/{}/merge",
                project_id(&repo.path),
                change.id
            ),
            &headers(token),
            &[],
            Some(&payload),
        )?;
        match reply.status {
            200 if parse_json(&reply)?["state"].as_str() == Some("merged") => {
                Ok(MergeOutcome::Merged)
            }
            200 => Ok(MergeOutcome::Blocked(refusal_reason(&reply))),
            // 405/406: not mergeable; 409: head moved since the expected sha.
            405 | 406 | 409 => Ok(MergeOutcome::Blocked(refusal_reason(&reply))),
            _ => Err(unexpected(&reply)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subgroup_paths_are_encoded_as_one_segment() {
        assert_eq!(project_id("group/sub/name"), "group%2Fsub%2Fname");
        assert_eq!(project_id("name"), "name");
    }

    #[test]
    fn pipeline_statuses_map_to_ci_states() {
        assert_eq!(pipeline_state("success"), Some(CiState::Success));
        assert_eq!(pipeline_state("failed"), Some(CiState::Failure));
        assert_eq!(pipeline_state("canceled"), Some(CiState::Failure));
        assert_eq!(pipeline_state("skipped"), None);
        assert_eq!(pipeline_state("manual"), Some(CiState::Blocked));
        assert_eq!(pipeline_state("running"), Some(CiState::Pending));
        assert_eq!(pipeline_state("created"), Some(CiState::Pending));
        assert_eq!(pipeline_state("something-new"), Some(CiState::Pending));
    }
}
