//! The report and its schema.

use std::collections::BTreeMap;

use fleetify::events::Redactor;
use fleetify::report::{Failure, RepoResult, build, json_schema};

#[test]
fn report_validates_against_its_own_schema() {
    let mut failing = RepoResult::new(
        "gitlab.com/group/sub/x",
        "verify_failed",
        "verifier exited with 1",
    );
    failing.failure = Some(Failure {
        class: "verify_failed".into(),
        message: "verifier exited with 1".into(),
        rule: None,
        output_tail: vec!["E  missing file".into()],
        artifacts: BTreeMap::from([("events".to_string(), "/w/events.jsonl".to_string())]),
        next_step: None,
    });
    let report = build(
        "run-1",
        "recipe",
        "2026-10-18T10:00:00Z",
        "2026-10-18T10:05:00Z",
        vec![
            RepoResult::new("github.com/a/b", "dry_run", "would branch"),
            failing,
            RepoResult::new("github.com/c/d", "error", ""),
        ],
        &Redactor::default(),
    );
    assert_eq!(report.exit_code, 1);

    let schema: serde_json::Value = serde_json::from_str(&json_schema()).unwrap();
    let instance = serde_json::to_value(&report).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(&instance)
        .map(|e| e.to_string())
        .collect();
    assert!(errors.is_empty(), "{errors:?}");

    // Every unsuccessful repo has a failure block with a message.
    for repo in report.repos.iter().filter(|r| !r.successful) {
        let failure = repo.failure.as_ref().expect("failure block");
        assert!(!failure.message.is_empty(), "{}", repo.repo);
    }
}
