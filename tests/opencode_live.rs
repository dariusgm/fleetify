//! Live check of the verbatim-verifier rule with the real opencode CLI. Not run by default: it
//! makes one model call. Run it on purpose:
//!
//!     FLEETIFY_LIVE=1 FLEETIFY_OPENCODE_BIN=~/.opencode/bin/opencode \
//!         cargo test --test opencode_live -- --ignored --nocapture
//!
//! `FLEETIFY_LIVE_MODEL` picks the model (default `openai/gpt-5-nano`). The test uses the same
//! path as a real run: the task file from `write_task`, the grants from `invocation`, and the
//! runner. It passes when the verifier ran and the agent never called it through a shell wrapper.

use std::fs;
use std::path::PathBuf;

use fleetify::agents::{RESULT_FILE, invocation, write_task};
use fleetify::config::Permissions;
use fleetify::events::{EventBus, Redactor};
use fleetify::recipe::Recipe;
use fleetify::runner::{RunRequest, Runner};

const VERIFIED: &str = ".agent/verified";

fn git(dir: &std::path::Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fleetify-live-opencode-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
#[ignore = "live: one model call; FLEETIFY_LIVE=1 and FLEETIFY_OPENCODE_BIN must be set"]
fn opencode_runs_the_verifier_unwrapped() {
    if std::env::var("FLEETIFY_LIVE").is_err() {
        eprintln!("skipped: set FLEETIFY_LIVE=1 to run the live check");
        return;
    }
    let model = std::env::var("FLEETIFY_LIVE_MODEL").unwrap_or_else(|_| "openai/gpt-5-nano".into());

    let base = scratch();
    let recipe_dir = base.join("recipe");
    fs::create_dir_all(recipe_dir.join("tools")).unwrap();
    fs::write(
        recipe_dir.join("tools/verify.sh"),
        "#!/bin/sh\nmkdir -p .agent\ntouch .agent/verified\necho verifier-ran\n",
    )
    .unwrap();
    let recipe = Recipe::parse(
        r#"schema = 1
name = "opencode-live"
branch = "live"
pr_title = "live"
agent = "opencode"
tools = "tools"

[[instructions]]
text = "Create a file named hello.txt containing the word hi. Then run the verifier."

[needs]
commands = ["sh .agent/tools/verify.sh"]

[verify]
argv = ["sh", "{tools}/verify.sh", "{repo}"]
"#,
    )
    .unwrap();

    // A git repository with `.agent/` excluded, as a real workspace is set up.
    let workspace = base.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    git(&workspace, &["init", "-q"]);
    fs::write(workspace.join("README.md"), "scratch repository\n").unwrap();
    git(&workspace, &["add", "README.md"]);
    git(
        &workspace,
        &[
            "-c",
            "user.name=live",
            "-c",
            "user.email=live@example.invalid",
            "commit",
            "-q",
            "-m",
            "initial",
        ],
    );
    fs::write(workspace.join(".git/info/exclude"), ".agent/\n").unwrap();
    write_task(&workspace, &recipe_dir, &recipe).unwrap();
    let permissions = Permissions {
        commands: vec!["sh .agent/tools/verify.sh".into()],
        ..Default::default()
    };
    let agent = invocation("opencode", &recipe, &permissions, Some(&model), &workspace).unwrap();

    let runner = Runner::new(EventBus::new(), Redactor::default());
    let mut request = RunRequest::new("live", agent.argv);
    request.cwd = Some(workspace.clone());
    request.timeout = Some(std::time::Duration::from_secs(600));
    request.env = agent.env;
    let result = runner.run(request);

    let output: Vec<String> = result.lines.iter().map(|line| line.text.clone()).collect();
    for line in &output {
        eprintln!("| {line}");
    }
    let wrapped = output
        .iter()
        .any(|line| line.to_lowercase().contains("bash -lc"));
    let verifier_called = output
        .iter()
        .any(|line| line.contains("sh .agent/tools/verify.sh"));

    assert!(
        workspace.join(VERIFIED).exists(),
        "the verifier did not run; output above (agent result file: {RESULT_FILE})"
    );
    assert!(
        verifier_called && !wrapped,
        "the agent called the verifier through a shell wrapper or not at all"
    );
    let _ = fs::remove_dir_all(&base);
}
