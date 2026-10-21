//! GitHub REST API (v3). Docs: <https://docs.github.com/en/rest> (checked 2026-10).
//! Endpoints used: `GET /repos/{owner}/{repo}`, `GET|POST /repos/{o}/{r}/pulls`,
//! `PUT /repos/{o}/{r}/pulls/{number}/merge`.

use reqwest::Method;
use serde_json::{Value, json};

use super::{
    Access, ChangeRequest, CiItem, CiState, CrState, Host, HostError, Http, MergeOutcome, Reply,
    access_from_status, parse_json, refusal_reason, unexpected,
};
use crate::credentials::Secret;
use crate::events::EventBus;
use crate::repos::Repo;

/// Pages of 100 items, at most this many per list.
const MAX_PAGES: u32 = 50;

pub struct GitHub {
    http: Http,
}

impl GitHub {
    pub fn new(http: Http) -> Self {
        GitHub { http }
    }

    /// Every item of a paged GitHub list: `key` names the array in the reply, `total_count`
    /// says how many exist. Pages stop at `MAX_PAGES`, so a host that keeps reporting more
    /// cannot make the poll loop forever.
    fn all_pages(&self, path: &str, token: &Secret, key: &str) -> Result<Vec<Value>, HostError> {
        let mut items = Vec::new();
        for page in 1..=MAX_PAGES {
            let page_text = page.to_string();
            let reply = self.http.send(
                Method::GET,
                path,
                &headers(token),
                &[("per_page", "100"), ("page", &page_text)],
                None,
            )?;
            if reply.status != 200 {
                return Err(unexpected(&reply));
            }
            let body = parse_json(&reply)?;
            let list = body[key]
                .as_array()
                .ok_or_else(|| HostError::Parse(format!("{key} is not an array")))?;
            items.extend(list.iter().cloned());
            let total = body["total_count"].as_u64().unwrap_or(0);
            if list.is_empty() || items.len() as u64 >= total {
                break;
            }
        }
        Ok(items)
    }
}

/// `(owner, name)` of a GitHub repo path (`owner/name`).
fn owner_and_name(repo: &Repo) -> Result<(&str, &str), HostError> {
    repo.path
        .split_once('/')
        .ok_or_else(|| HostError::Parse(format!("{} is not owner/name", repo.path)))
}

fn headers(token: &Secret) -> Vec<(&'static str, String)> {
    vec![
        ("Authorization", format!("Bearer {}", token.expose())),
        ("Accept", "application/vnd.github+json".to_string()),
        ("X-GitHub-Api-Version", "2022-11-28".to_string()),
    ]
}

/// A check run's state. Completed with success, neutral or skipped counts as success;
/// anything else that completed is a failure; anything not completed is pending.
fn check_run_state(run: &Value) -> CiState {
    if run["status"].as_str() != Some("completed") {
        return CiState::Pending;
    }
    match run["conclusion"].as_str() {
        Some("success" | "neutral" | "skipped") => CiState::Success,
        Some("action_required") => CiState::Blocked,
        _ => CiState::Failure,
    }
}

/// Refuses an answer that names another commit than the one asked for.
fn check_commit(answered: Option<&str>, expected: &str) -> Result<(), HostError> {
    match answered {
        Some(sha) if sha == expected => Ok(()),
        other => Err(HostError::Parse(format!(
            "asked for commit {expected}, the host answered for {other:?}"
        ))),
    }
}

fn change_request(value: &Value) -> Result<ChangeRequest, HostError> {
    let number = value["number"]
        .as_u64()
        .ok_or_else(|| HostError::Parse("pull request has no number".into()))?;
    let state = match value["state"].as_str() {
        Some("open") => CrState::Open,
        Some("closed") if value["merged_at"].is_string() => CrState::Merged,
        Some("closed") => CrState::Closed,
        other => {
            return Err(HostError::Parse(format!(
                "unknown pull request state {other:?}"
            )));
        }
    };
    Ok(ChangeRequest {
        url: value["html_url"].as_str().unwrap_or_default().to_string(),
        id: number.to_string(),
        head_sha: value["head"]["sha"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        state,
    })
}

impl Host for GitHub {
    fn attach_events(&self, bus: &EventBus) {
        self.http.attach_events(bus);
    }

    fn probe_read_access(&self, repo: &Repo, token: &Secret) -> Result<Access, HostError> {
        let (owner, name) = owner_and_name(repo)?;
        let reply = self.http.send(
            Method::GET,
            &format!("/repos/{owner}/{name}"),
            &headers(token),
            &[],
            None,
        )?;
        access_from_status(reply.status).ok_or_else(|| unexpected(&reply))
    }

    fn default_branch(&self, repo: &Repo, token: &Secret) -> Result<String, HostError> {
        let (owner, name) = owner_and_name(repo)?;
        let reply = self.http.send(
            Method::GET,
            &format!("/repos/{owner}/{name}"),
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
            .ok_or_else(|| HostError::Parse("repository has no default_branch".into()))
    }

    fn latest_change_request(
        &self,
        repo: &Repo,
        token: &Secret,
        branch: &str,
    ) -> Result<Option<ChangeRequest>, HostError> {
        let (owner, name) = owner_and_name(repo)?;
        let head = format!("{owner}:{branch}");
        let reply = self.http.send(
            Method::GET,
            &format!("/repos/{owner}/{name}/pulls"),
            &headers(token),
            &[
                ("state", "all"),
                ("head", &head),
                ("sort", "created"),
                ("direction", "desc"),
                ("per_page", "1"),
            ],
            None,
        )?;
        if reply.status != 200 {
            return Err(unexpected(&reply));
        }
        let list = parse_json(&reply)?;
        list.as_array()
            .ok_or_else(|| HostError::Parse("pull list is not an array".into()))?
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
        let (owner, name) = owner_and_name(repo)?;
        let head = format!("{owner}:{branch}");
        let reply = self.http.send(
            Method::GET,
            &format!("/repos/{owner}/{name}/pulls"),
            &headers(token),
            &[("state", "open"), ("head", &head)],
            None,
        )?;
        if reply.status != 200 {
            return Err(unexpected(&reply));
        }
        let list = parse_json(&reply)?;
        let items = list
            .as_array()
            .ok_or_else(|| HostError::Parse("pull list is not an array".into()))?;
        // Only open ones count, even if the host returned others.
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
        let (owner, name) = owner_and_name(repo)?;
        let payload = json!({ "title": title, "head": branch, "base": base, "body": body });
        let reply = self.http.send(
            Method::POST,
            &format!("/repos/{owner}/{name}/pulls"),
            &headers(token),
            &[],
            Some(&payload),
        )?;
        if reply.status != 201 {
            return Err(unexpected(&reply));
        }
        change_request(&parse_json(&reply)?)
    }

    fn commit_ci(&self, repo: &Repo, token: &Secret, sha: &str) -> Result<Vec<CiItem>, HostError> {
        let (owner, name) = owner_and_name(repo)?;
        let mut items = Vec::new();

        // Check runs (GitHub Actions and apps). Every item must name this exact commit.
        let runs = self.all_pages(
            &format!("/repos/{owner}/{name}/commits/{sha}/check-runs"),
            token,
            "check_runs",
        )?;
        for run in &runs {
            check_commit(run["head_sha"].as_str(), sha)?;
            items.push(CiItem {
                name: run["name"].as_str().unwrap_or_default().to_string(),
                state: check_run_state(run),
            });
        }

        // Commit statuses (older CI integrations). Same rule: the commit must match.
        let reply = self.http.send(
            Method::GET,
            &format!("/repos/{owner}/{name}/commits/{sha}/status"),
            &headers(token),
            &[],
            None,
        )?;
        if reply.status != 200 {
            return Err(unexpected(&reply));
        }
        let combined = parse_json(&reply)?;
        check_commit(combined["sha"].as_str(), sha)?;
        let statuses = self.all_pages(
            &format!("/repos/{owner}/{name}/commits/{sha}/status"),
            token,
            "statuses",
        )?;
        for status in &statuses {
            items.push(CiItem {
                name: status["context"].as_str().unwrap_or_default().to_string(),
                state: match status["state"].as_str() {
                    Some("success") => CiState::Success,
                    Some("pending") => CiState::Pending,
                    _ => CiState::Failure,
                },
            });
        }
        Ok(items)
    }

    fn delete_branch(&self, repo: &Repo, token: &Secret, branch: &str) -> Result<(), HostError> {
        let (owner, name) = owner_and_name(repo)?;
        let reply: Reply = self.http.send(
            Method::DELETE,
            &format!("/repos/{owner}/{name}/git/refs/heads/{branch}"),
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
        let (owner, name) = owner_and_name(repo)?;
        let payload = json!({ "sha": expected_sha, "merge_method": "merge" });
        let reply: Reply = self.http.send(
            Method::PUT,
            &format!("/repos/{owner}/{name}/pulls/{}/merge", change.id),
            &headers(token),
            &[],
            Some(&payload),
        )?;
        match reply.status {
            200 if parse_json(&reply)?["merged"].as_bool() == Some(true) => {
                Ok(MergeOutcome::Merged)
            }
            200 => Ok(MergeOutcome::Blocked(refusal_reason(&reply))),
            // 405: not mergeable; 409: head moved since the expected sha.
            405 | 409 => Ok(MergeOutcome::Blocked(refusal_reason(&reply))),
            _ => Err(unexpected(&reply)),
        }
    }
}
