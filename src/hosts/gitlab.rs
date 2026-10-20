//! GitLab REST API (v4). Docs: <https://docs.gitlab.com/api/rest/> (checked 2026-10).
//! Endpoints used: `GET /projects/:id`, `GET|POST /projects/:id/merge_requests`,
//! `PUT /projects/:id/merge_requests/:iid/merge`. `:id` is the URL-encoded full path.

use reqwest::Method;
use serde_json::{Value, json};

use super::{
    Access, ChangeRequest, CiItem, CrState, Host, HostError, Http, MergeOutcome, Reply,
    access_from_status, parse_json, refusal_reason, unexpected,
};
use crate::credentials::Secret;
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

impl Host for GitLab {
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

    fn commit_ci(
        &self,
        _repo: &Repo,
        _token: &Secret,
        _sha: &str,
    ) -> Result<Vec<CiItem>, HostError> {
        Err(HostError::Unsupported(
            "commit CI status for GitLab (M11, GitLab CI is skipped for now)".into(),
        ))
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
}
