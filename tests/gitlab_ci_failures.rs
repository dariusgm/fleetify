//! GitLab's failed jobs for one commit, nested pipelines included: a trigger job and a
//! job that may fail are left out, the child pipeline's failing job is reported with the end
//! of its trace. A fake GitLab answers the pipeline, bridge, job and trace requests.

mod support;

use fleetify::config::HostType;
use fleetify::credentials::Secret;
use fleetify::hosts::{LogBudget, client_for};
use fleetify::repos::Repo;
use support::{Recorded, serve};

const SHA: &str = "abc123";

fn fake() -> support::FakeHost {
    serve(|req: &Recorded| {
        // The test passes the server root as the API base, so requests carry no /api/v4 prefix.
        let path = req.target.split('?').next().unwrap().replace("/api/v4", "");
        match path.as_str() {
            "/projects/g%2Fp/pipelines" => (200, r#"[{"id":1,"sha":"abc123"}]"#.into()),
            // Pipeline 1 triggers pipeline 2 through job 5.
            "/projects/g%2Fp/pipelines/1/bridges" => (
                200,
                r#"[{"id":5,"downstream_pipeline":{"id":2}}]"#.into(),
            ),
            "/projects/g%2Fp/pipelines/1/jobs" => (
                200,
                r#"[{"id":5,"name":"trigger-tests","status":"failed","allow_failure":false,"started_at":"2026-10-22T10:00:00Z"},
                    {"id":6,"name":"lint","status":"success","allow_failure":false,"started_at":"2026-10-22T10:00:00Z"}]"#
                    .into(),
            ),
            "/projects/g%2Fp/pipelines/2/bridges" => (200, "[]".into()),
            "/projects/g%2Fp/pipelines/2/jobs" => (
                200,
                r#"[{"id":22,"name":"flaky-optional","status":"failed","allow_failure":true,"started_at":"2026-10-22T10:01:00Z"},
                    {"id":21,"name":"unit","status":"failed","allow_failure":false,"started_at":"2026-10-22T10:02:00Z"}]"#
                    .into(),
            ),
            "/projects/g%2Fp/jobs/21/trace" => (
                200,
                "collecting\nFAILED tests/test_x.py::test_y\nE   AssertionError: 3 != 4\n".into(),
            ),
            _ => (404, r#"{"message":"404 Not Found"}"#.into()),
        }
    })
}

#[test]
fn nested_failures_are_listed_root_first_without_triggers_or_allowed_failures() {
    let host = fake();
    let client = client_for(HostType::Gitlab, "gitlab.example.com", Some(&host.base)).unwrap();
    let repo = Repo {
        host: "gitlab.example.com".into(),
        host_type: HostType::Gitlab,
        path: "g/p".into(),
        name: "p".into(),
        clone_url: "https://gitlab.example.com/g/p.git".into(),
        line: 1,
    };
    let token = Secret::new("tok-gitlab-failures");
    let failures = client
        .ci_failures(
            &repo,
            &token,
            SHA,
            LogBudget {
                lines: 2,
                chars: 1000,
            },
        )
        .unwrap();
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert_eq!(failures[0].name, "unit");
    assert_eq!(
        failures[0].log_tail,
        vec![
            "FAILED tests/test_x.py::test_y",
            "E   AssertionError: 3 != 4"
        ]
    );
}
