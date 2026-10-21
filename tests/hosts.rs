//! GitHub and GitLab backends against a fake host server.

mod support;

use fleetify::config::HostType;
use fleetify::credentials::Secret;
use fleetify::hosts::client_for_policy;
use fleetify::hosts::{
    Access, CiItem, CiState, CrState, Host, HostError, MergeOutcome, client_for,
};
use fleetify::repos::Repo;
use std::time::{Duration, Instant};
use support::{Recorded, serve, serve_with_headers};

const TOKEN: &str = "tok-host-test-123";

fn repo(host: &str, host_type: HostType, path: &str) -> Repo {
    Repo {
        host: host.into(),
        host_type,
        path: path.into(),
        name: path.rsplit('/').next().unwrap().into(),
        clone_url: String::new(),
        line: 1,
    }
}

fn github(base: &str) -> Box<dyn Host> {
    client_for(HostType::Github, "github.com", Some(base)).unwrap()
}

fn gitlab(base: &str) -> Box<dyn Host> {
    client_for(HostType::Gitlab, "gitlab.example.com", Some(base)).unwrap()
}

fn token() -> Secret {
    Secret::new(TOKEN)
}

/// The token must never appear in a URL or a body, only in auth headers.
fn assert_token_only_in_headers(request: &Recorded) {
    assert!(
        !request.target.contains(TOKEN),
        "token in URL: {}",
        request.target
    );
    assert!(!request.body.contains(TOKEN), "token in body");
}

#[test]
fn github_probe_maps_every_status() {
    let host = serve(|req| match req.target.as_str() {
        "/repos/o/ok" => (200, r#"{"default_branch":"main"}"#.into()),
        "/repos/o/denied" => (403, r#"{"message":"Forbidden"}"#.into()),
        "/repos/o/missing" => (404, r#"{"message":"Not Found"}"#.into()),
        _ => (401, r#"{"message":"Bad credentials"}"#.into()),
    });
    let client = github(&host.base);
    assert_eq!(
        client
            .probe_read_access(&repo("github.com", HostType::Github, "o/ok"), &token())
            .unwrap(),
        Access::Ok
    );
    assert_eq!(
        client
            .probe_read_access(&repo("github.com", HostType::Github, "o/denied"), &token())
            .unwrap(),
        Access::Forbidden
    );
    assert_eq!(
        client
            .probe_read_access(&repo("github.com", HostType::Github, "o/missing"), &token())
            .unwrap(),
        Access::NotFound
    );
    assert_eq!(
        client
            .probe_read_access(&repo("github.com", HostType::Github, "o/other"), &token())
            .unwrap(),
        Access::Unauthorized
    );

    let requests = host.recorded();
    assert_eq!(
        requests[0].header("Authorization"),
        Some(format!("Bearer {TOKEN}").as_str())
    );
    assert_eq!(
        requests[0].header("Accept"),
        Some("application/vnd.github+json")
    );
    requests.iter().for_each(assert_token_only_in_headers);
}

#[test]
fn github_default_branch_is_read_from_the_repository() {
    let host = serve(|_| (200, r#"{"default_branch":"trunk"}"#.into()));
    let branch = github(&host.base)
        .default_branch(&repo("github.com", HostType::Github, "o/r"), &token())
        .unwrap();
    assert_eq!(branch, "trunk");
}

#[test]
fn github_finds_only_open_pull_requests() {
    let host = serve(|_| {
        (
            200,
            r#"[{"number":7,"state":"closed","merged_at":null,"html_url":"https://x/7","head":{"sha":"aaa"}}]"#.into(),
        )
    });
    let found = github(&host.base)
        .find_change_request(
            &repo("github.com", HostType::Github, "o/r"),
            &token(),
            "chore/x",
        )
        .unwrap();
    assert_eq!(found, None, "a closed pull request is not open");
    let query = &host.recorded()[0].target;
    assert!(query.contains("state=open"), "{query}");
    assert!(
        query.contains("head=o%3Achore%2Fx") || query.contains("head=o:chore/x"),
        "{query}"
    );
}

#[test]
fn github_creates_a_pull_request_with_branch_base_title_and_body() {
    let host = serve(|_| {
        (
            201,
            r#"{"number":12,"state":"open","html_url":"https://github.com/o/r/pull/12","head":{"sha":"bbb"}}"#.into(),
        )
    });
    let change = github(&host.base)
        .create_change_request(
            &repo("github.com", HostType::Github, "o/r"),
            &token(),
            "chore/x",
            "main",
            "T",
            "B",
        )
        .unwrap();
    assert_eq!(change.id, "12");
    assert_eq!(change.head_sha, "bbb");
    assert_eq!(change.state, CrState::Open);
    let sent: serde_json::Value = serde_json::from_str(&host.recorded()[0].body).unwrap();
    assert_eq!(sent["head"], "chore/x");
    assert_eq!(sent["base"], "main");
    assert_eq!(sent["title"], "T");
    assert_token_only_in_headers(&host.recorded()[0]);
}

#[test]
fn github_merge_is_guarded_by_the_expected_head_and_refusals_block() {
    let host = serve(|req| {
        if req.body.contains("\"sha\":\"good\"") {
            (200, r#"{"merged":true}"#.into())
        } else {
            (
                409,
                r#"{"message":"Head branch was modified. Review and try the merge again."}"#.into(),
            )
        }
    });
    let client = github(&host.base);
    let change = fleetify::hosts::ChangeRequest {
        url: String::new(),
        id: "12".into(),
        head_sha: "good".into(),
        state: CrState::Open,
    };
    let repo = repo("github.com", HostType::Github, "o/r");
    assert_eq!(
        client.merge(&repo, &token(), &change, "good").unwrap(),
        MergeOutcome::Merged
    );
    match client.merge(&repo, &token(), &change, "stale").unwrap() {
        MergeOutcome::Blocked(reason) => {
            assert!(reason.contains("Head branch was modified"), "{reason}")
        }
        other => panic!("expected a block, got {other:?}"),
    }
    assert!(host.recorded()[1].body.contains("\"sha\":\"stale\""));
}

#[test]
fn delete_branch_treats_a_missing_branch_as_deleted_and_surfaces_refusals() {
    let host = serve(|req| {
        if req.target.ends_with("/git/refs/heads/chore/gone") {
            (404, r#"{"message":"Not Found"}"#.into())
        } else if req.target.ends_with("/git/refs/heads/chore/x") {
            (204, String::new())
        } else {
            (403, r#"{"message":"Forbidden"}"#.into())
        }
    });
    let client = github(&host.base);
    let repo = repo("github.com", HostType::Github, "o/r");
    client.delete_branch(&repo, &token(), "chore/x").unwrap();
    client.delete_branch(&repo, &token(), "chore/gone").unwrap();
    assert!(client.delete_branch(&repo, &token(), "main").is_err());
    assert!(host.recorded()[0].method == "DELETE");
    assert_token_only_in_headers(&host.recorded()[0]);
}

#[test]
fn gitlab_delete_branch_encodes_the_branch_name_as_one_segment() {
    let host = serve(|_| (204, String::new()));
    let client = gitlab(&host.base);
    let repo = repo("gitlab.com", HostType::Gitlab, "team/project");
    client.delete_branch(&repo, &token(), "chore/x").unwrap();
    let target = &host.recorded()[0].target;
    assert_eq!(
        target, "/projects/team%2Fproject/repository/branches/chore%2Fx",
        "{target}"
    );
}

#[test]
fn unexpected_status_is_an_error_not_a_guess() {
    let host = serve(|_| (500, r#"{"message":"boom"}"#.into()));
    let err = github(&host.base)
        .default_branch(&repo("github.com", HostType::Github, "o/r"), &token())
        .unwrap_err();
    assert_eq!(
        err,
        HostError::Status {
            status: 500,
            message: "boom".into()
        }
    );
}

#[test]
fn gitlab_project_paths_with_subgroups_are_url_encoded() {
    let host = serve(|_| (200, r#"{"default_branch":"main"}"#.into()));
    let access = gitlab(&host.base)
        .probe_read_access(
            &repo("gitlab.example.com", HostType::Gitlab, "team/sub/project-c"),
            &token(),
        )
        .unwrap();
    assert_eq!(access, Access::Ok);
    let request = &host.recorded()[0];
    assert_eq!(request.target, "/projects/team%2Fsub%2Fproject-c");
    assert_eq!(request.header("PRIVATE-TOKEN"), Some(TOKEN));
    assert_token_only_in_headers(request);
}

#[test]
fn gitlab_finds_creates_and_merges_merge_requests() {
    let host = serve(|req| {
        if req.method == "GET" {
            (200, "[]".into())
        } else if req.method == "POST" {
            (
                201,
                r#"{"iid":3,"state":"opened","web_url":"https://gl/x/3","sha":"ccc"}"#.into(),
            )
        } else if req.body.contains("\"sha\":\"ccc\"") {
            (200, r#"{"state":"merged"}"#.into())
        } else {
            (
                409,
                r#"{"message":"SHA does not match HEAD of source branch"}"#.into(),
            )
        }
    });
    let client = gitlab(&host.base);
    let repo = repo("gitlab.example.com", HostType::Gitlab, "team/sub/p");

    assert_eq!(
        client
            .find_change_request(&repo, &token(), "chore/x")
            .unwrap(),
        None
    );
    let query = &host.recorded()[0].target;
    assert!(
        query.contains("state=opened") && query.contains("source_branch=chore%2Fx"),
        "{query}"
    );

    let change = client
        .create_change_request(&repo, &token(), "chore/x", "main", "T", "B")
        .unwrap();
    assert_eq!(change.id, "3");
    assert_eq!(change.head_sha, "ccc");

    assert_eq!(
        client.merge(&repo, &token(), &change, "ccc").unwrap(),
        MergeOutcome::Merged
    );
    match client.merge(&repo, &token(), &change, "old").unwrap() {
        MergeOutcome::Blocked(reason) => assert!(reason.contains("SHA does not match"), "{reason}"),
        other => panic!("expected a block, got {other:?}"),
    }
    let merge_request = host
        .recorded()
        .into_iter()
        .find(|r| r.target.ends_with("/merge"))
        .unwrap();
    assert_eq!(
        merge_request.target,
        "/projects/team%2Fsub%2Fp/merge_requests/3/merge"
    );
    requests_have_no_token_in_url(&host.recorded());
}

fn requests_have_no_token_in_url(requests: &[Recorded]) {
    requests.iter().for_each(assert_token_only_in_headers);
}

#[test]
fn github_returns_an_existing_open_pull_request() {
    let host = serve(|_| {
        (
            200,
            r#"[{"number":9,"state":"open","merged_at":null,"html_url":"https://github.com/o/r/pull/9","head":{"sha":"ddd"}}]"#
                .into(),
        )
    });
    let found = github(&host.base)
        .find_change_request(
            &repo("github.com", HostType::Github, "o/r"),
            &token(),
            "chore/x",
        )
        .unwrap()
        .expect("the open pull request");
    assert_eq!(found.id, "9");
    assert_eq!(found.head_sha, "ddd");
    assert_eq!(found.state, CrState::Open);
}

#[test]
fn gitlab_ignores_a_closed_merge_request_even_if_listed() {
    let host = serve(|_| {
        (
            200,
            r#"[{"iid":4,"state":"closed","web_url":"https://gl/x/4","sha":"eee"}]"#.into(),
        )
    });
    let found = gitlab(&host.base)
        .find_change_request(
            &repo("gitlab.example.com", HostType::Gitlab, "a/b"),
            &token(),
            "chore/x",
        )
        .unwrap();
    assert_eq!(found, None);
}

#[test]
fn github_merge_refused_by_the_host_is_a_block_with_its_reason() {
    let host = serve(|_| (405, r#"{"message":"Pull Request is not mergeable"}"#.into()));
    let change = fleetify::hosts::ChangeRequest {
        url: String::new(),
        id: "12".into(),
        head_sha: "x".into(),
        state: CrState::Open,
    };
    match github(&host.base)
        .merge(
            &repo("github.com", HostType::Github, "o/r"),
            &token(),
            &change,
            "x",
        )
        .unwrap()
    {
        MergeOutcome::Blocked(reason) => assert!(reason.contains("not mergeable"), "{reason}"),
        other => panic!("expected a block, got {other:?}"),
    }
}

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const OTHER_SHA: &str = "fedcba9876543210fedcba9876543210fedcba98";

#[test]
fn commit_ci_reads_check_runs_and_statuses_of_the_asked_commit() {
    let host = serve(|req| {
        if req.target.ends_with("/check-runs?per_page=100") {
            (
                200,
                format!(
                    r#"{{"check_runs":[
                        {{"name":"lint","head_sha":"{SHA}","status":"completed","conclusion":"success"}},
                        {{"name":"test","head_sha":"{SHA}","status":"in_progress","conclusion":null}}
                    ]}}"#
                ),
            )
        } else if req.target.ends_with("/status") {
            (
                200,
                format!(
                    r#"{{"sha":"{SHA}","statuses":[{{"context":"jenkins","state":"failure"}}]}}"#
                ),
            )
        } else {
            (404, "{}".into())
        }
    });
    let items = github(&host.base)
        .commit_ci(&repo("github.com", HostType::Github, "o/r"), &token(), SHA)
        .unwrap();
    assert_eq!(
        items,
        vec![
            CiItem {
                name: "lint".into(),
                state: CiState::Success
            },
            CiItem {
                name: "test".into(),
                state: CiState::Pending
            },
            CiItem {
                name: "jenkins".into(),
                state: CiState::Failure
            },
        ]
    );
}

#[test]
fn commit_ci_refuses_items_of_another_commit() {
    // A check run for a later push must never decide the verdict for this commit.
    let host = serve(|req| {
        if req.target.ends_with("/check-runs?per_page=100") {
            (
                200,
                format!(
                    r#"{{"check_runs":[{{"name":"test","head_sha":"{OTHER_SHA}","status":"completed","conclusion":"success"}}]}}"#
                ),
            )
        } else {
            (200, format!(r#"{{"sha":"{SHA}","statuses":[]}}"#))
        }
    });
    let err = github(&host.base)
        .commit_ci(&repo("github.com", HostType::Github, "o/r"), &token(), SHA)
        .unwrap_err();
    assert!(matches!(err, HostError::Parse(_)), "{err:?}");
}

#[test]
fn gitlab_commit_ci_reads_the_pipelines_of_the_asked_commit() {
    let host = serve(|req| {
        assert!(req.target.contains("/projects/group%2Fproject/pipelines?"));
        assert!(req.target.contains(&format!("sha={SHA}")), "{}", req.target);
        (
            200,
            format!(
                r#"[
                    {{"id":11,"sha":"{SHA}","ref":"feature","status":"success"}},
                    {{"id":12,"sha":"{SHA}","ref":"feature","status":"running"}},
                    {{"id":13,"sha":"{SHA}","ref":"feature","status":"skipped"}}
                ]"#
            ),
        )
    });
    let items = gitlab(&host.base)
        .commit_ci(
            &repo("gitlab.example.com", HostType::Gitlab, "group/project"),
            &token(),
            SHA,
        )
        .unwrap();
    assert_eq!(
        items,
        vec![
            CiItem {
                name: "pipeline 11 (feature)".into(),
                state: CiState::Success
            },
            CiItem {
                name: "pipeline 12 (feature)".into(),
                state: CiState::Pending
            },
        ]
    );
}

#[test]
fn gitlab_commit_ci_refuses_pipelines_of_another_commit() {
    let host = serve(|_| {
        (
            200,
            format!(r#"[{{"id":11,"sha":"{OTHER_SHA}","ref":"main","status":"failed"}}]"#),
        )
    });
    let err = gitlab(&host.base)
        .commit_ci(
            &repo("gitlab.example.com", HostType::Gitlab, "group/project"),
            &token(),
            SHA,
        )
        .unwrap_err();
    assert!(matches!(err, HostError::Parse(_)), "{err:?}");
}

#[test]
fn gitlab_commit_ci_with_only_skipped_pipelines_has_no_items() {
    let host = serve(|_| {
        (
            200,
            format!(r#"[{{"id":11,"sha":"{SHA}","ref":"main","status":"skipped"}}]"#),
        )
    });
    let items = gitlab(&host.base)
        .commit_ci(
            &repo("gitlab.example.com", HostType::Gitlab, "group/project"),
            &token(),
            SHA,
        )
        .unwrap();
    assert!(items.is_empty());
}

#[test]
fn gitlab_latest_change_request_returns_a_merged_one_for_the_branch() {
    let host = serve(|req| {
        assert!(req.target.contains("state=all"), "{}", req.target);
        assert!(
            req.target.contains("source_branch=fleetify%2Fchore%2Fx")
                || req.target.contains("source_branch=fleetify/chore/x"),
            "{}",
            req.target
        );
        (
            200,
            r#"[{"iid":7,"state":"merged","web_url":"https://gitlab.example.com/g/p/-/merge_requests/7","sha":"abc"}]"#
                .into(),
        )
    });
    let change = gitlab(&host.base)
        .latest_change_request(
            &repo("gitlab.example.com", HostType::Gitlab, "group/project"),
            &token(),
            "fleetify/chore/x",
        )
        .unwrap()
        .expect("a change request");
    assert_eq!(change.id, "7");
    assert_eq!(change.state, CrState::Merged);
}

fn github_with_ceiling(base: &str, max_wait: Duration) -> Box<dyn Host> {
    client_for_policy(HostType::Github, "github.com", Some(base), max_wait).unwrap()
}

#[test]
fn a_retry_after_wait_is_taken_and_the_request_repeated() {
    let answers = std::sync::atomic::AtomicUsize::new(0);
    let host = serve_with_headers(move |_| {
        if answers.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            return (429, vec![("Retry-After".into(), "1".into())], "{}".into());
        }
        (200, Vec::new(), r#"{"default_branch":"main"}"#.into())
    });
    let client = github_with_ceiling(&host.base, Duration::from_secs(60));
    let started = Instant::now();
    let branch = client
        .default_branch(&repo("github.com", HostType::Github, "o/r"), &token())
        .unwrap();
    assert_eq!(branch, "main");
    assert!(
        started.elapsed() >= Duration::from_millis(900),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(host.recorded().len(), 2);
}

#[test]
fn a_wait_longer_than_the_ceiling_is_not_taken() {
    let host = serve_with_headers(|_| {
        (
            429,
            vec![("Retry-After".into(), "3600".into())],
            "{}".into(),
        )
    });
    let client = github_with_ceiling(&host.base, Duration::from_secs(60));
    let err = client
        .default_branch(&repo("github.com", HostType::Github, "o/r"), &token())
        .unwrap_err();
    match err {
        HostError::RateLimited { wait, .. } => assert_eq!(wait, Duration::from_secs(3600)),
        other => panic!("{other:?}"),
    }
    // One request only: the wait was not taken, so no retry was sent.
    assert_eq!(host.recorded().len(), 1);
}

#[test]
fn a_zero_ceiling_never_waits() {
    let host = serve_with_headers(|_| (429, vec![("Retry-After".into(), "1".into())], "{}".into()));
    let client = github_with_ceiling(&host.base, Duration::ZERO);
    assert!(
        client
            .default_branch(&repo("github.com", HostType::Github, "o/r"), &token())
            .unwrap_err()
            .is_rate_limited()
    );
    assert_eq!(host.recorded().len(), 1);
}

#[test]
fn an_exhausted_quota_holds_back_the_next_request_of_the_same_host() {
    // The first reply stands, but the quota is used up until the reset time. The reset is
    // given in whole seconds, so the second request must not come back before that instant.
    let reset_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 2;
    let reset = reset_at.to_string();
    let host = serve_with_headers(move |_| {
        (
            200,
            vec![
                ("x-ratelimit-remaining".into(), "0".into()),
                ("x-ratelimit-reset".into(), reset.clone()),
            ],
            r#"{"default_branch":"main"}"#.into(),
        )
    });
    let client = github_with_ceiling(&host.base, Duration::from_secs(60));
    let repo = repo("github.com", HostType::Github, "o/r");
    client.default_branch(&repo, &token()).unwrap();
    client.default_branch(&repo, &token()).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!(
        now >= reset_at,
        "second request came back at {now}, before the reset at {reset_at}"
    );
    assert_eq!(host.recorded().len(), 2);
}
