//! The judge call end to end with a fake opencode: one fresh call, an empty working directory
//! that holds only the input, no executor variables, and a verdict read from the output.
//! The fake records what it was given, so the test can check the call itself.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use fleetify::events::{EventBus, Redactor};
use fleetify::judge::{self, Verdict};
use fleetify::runner::Runner;

fn scratch(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("fleetify-judge-test-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A fake `opencode`: the `fake-opencode` helper binary, copied into `dir` with its settings
/// beside it. It records argv, working directory and environment in `log`, requires the input
/// file in its working directory, then prints `output`.
fn fake_opencode(dir: &Path, log: &Path, output: &str) -> PathBuf {
    let script = dir.join(format!("fake-opencode{}", env::consts::EXE_SUFFIX));
    fs::copy(env!("CARGO_BIN_EXE_fake-opencode"), &script).unwrap();
    fs::write(
        dir.join("fake-opencode.conf"),
        format!("{}\n{output}\n", log.display()),
    )
    .unwrap();
    script
}

fn runner() -> Runner {
    Runner::new(EventBus::new(), Redactor::new(Vec::<String>::new()))
}

fn the_judge_is_one_fresh_call_in_an_empty_directory_and_reads_the_verdict() {
    let dir = scratch("call");
    let log = dir.join("call.log");
    let script = fake_opencode(
        &dir,
        &log,
        r#"{"verdict": "fail", "reasons": ["weakened assertion"]}"#,
    );
    // SAFETY: this test binary runs no other test that reads this variable.
    unsafe {
        std::env::set_var("FLEETIFY_OPENCODE_BIN", &script);
        std::env::set_var("ANTHROPIC_API_KEY", "must-not-reach-the-judge");
        std::env::set_var("FLEETIFY_TEST_EXECUTOR_VARIABLE", "executor");
    }

    let input = judge::input_text("make the test pass", "verifier ok", &[], "+x\n");
    let call = judge::run_judge(
        &runner(),
        "judge-test",
        "opencode",
        "llamacpp/qwen3.8-flash-next",
        &input,
        std::time::Duration::from_secs(60),
    );
    let review = call.review.expect("the fake answers with a verdict");
    assert_eq!(review.verdict, Verdict::Fail);
    assert_eq!(review.reasons, vec!["weakened assertion".to_string()]);

    let recorded = fs::read_to_string(&log).unwrap();
    // The working directory is the judge's own, not the repository and not the test folder.
    let cwd = recorded
        .lines()
        .find_map(|l| l.strip_prefix("cwd="))
        .unwrap();
    assert_ne!(Path::new(cwd), dir.as_path());
    // Fresh: the arguments name no session to continue.
    for line in recorded.lines().filter_map(|l| l.strip_prefix("arg=")) {
        assert!(
            !matches!(
                line,
                "-c" | "--continue" | "-s" | "--session" | "--resume" | "-r"
            ),
            "{line}"
        );
    }
    assert!(recorded.contains("arg=--model"));
    assert!(recorded.contains("arg=llamacpp/qwen3.8-flash-next"));
    // No executor variable reaches the judge, even when the test process has it.
    assert!(!recorded.contains("ANTHROPIC_API_KEY"), "{recorded}");
    assert!(
        !recorded.contains("FLEETIFY_TEST_EXECUTOR_VARIABLE"),
        "{recorded}"
    );
    assert!(recorded.contains("OPENCODE_CONFIG_CONTENT"));
    // The input the judge read is the input it was given.
    assert!(recorded.contains("# Task\n\nmake the test pass"));
    // The judge directory is removed.
    assert!(!Path::new(cwd).exists());
    let _ = fs::remove_dir_all(&dir);
}

fn an_answer_without_a_verdict_is_an_error_the_pipeline_turns_into_unsure() {
    let dir = scratch("garbage");
    let log = dir.join("call.log");
    let script = fake_opencode(&dir, &log, "I think it is fine, probably");
    // SAFETY: as above, this test binary runs no other test that reads this variable.
    unsafe {
        std::env::set_var("FLEETIFY_OPENCODE_BIN", &script);
    }
    let call = judge::run_judge(
        &runner(),
        "judge-test",
        "opencode",
        "llamacpp/x",
        "# Task\n",
        std::time::Duration::from_secs(60),
    );
    assert!(call.review.is_err(), "no verdict object: {:?}", call.review);
    let _ = fs::remove_dir_all(&dir);
}

fn a_verdict_outside_the_three_answers_is_an_error_too() {
    let dir = scratch("invalid-verdict");
    let log = dir.join("call.log");
    let script = fake_opencode(&dir, &log, r#"{"verdict": "maybe", "reasons": ["unsure"]}"#);
    // SAFETY: as above, this test binary runs no other test that reads this variable.
    unsafe {
        std::env::set_var("FLEETIFY_OPENCODE_BIN", &script);
    }
    let call = judge::run_judge(
        &runner(),
        "judge-test",
        "opencode",
        "llamacpp/x",
        "# Task\n",
        std::time::Duration::from_secs(60),
    );
    assert!(
        call.review.is_err(),
        "a verdict other than pass, fail or unsure must not be accepted: {:?}",
        call.review
    );
    let _ = fs::remove_dir_all(&dir);
}

/// Both cases in one test: they share the `FLEETIFY_OPENCODE_BIN` variable, so they must not run
/// in parallel.
#[test]
fn judge_calls_follow_the_rules_with_a_fake_opencode() {
    the_judge_is_one_fresh_call_in_an_empty_directory_and_reads_the_verdict();
    an_answer_without_a_verdict_is_an_error_the_pipeline_turns_into_unsure();
    a_verdict_outside_the_three_answers_is_an_error_too();
}
