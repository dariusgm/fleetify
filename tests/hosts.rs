//! GitHub and GitLab backends against a fake host server.

mod support;

use fleetify::config::HostType;
use fleetify::credentials::Secret;
use fleetify::hosts::{Access, CrState, Host, HostError, MergeOutcome, client_for};
use fleetify::repos::Repo;
use support::{Recorded, serve};

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
