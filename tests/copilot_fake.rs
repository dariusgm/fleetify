//! `check-agent copilot` against fake `copilot` scripts, so the canary logic is tested
//! without a Copilot account. Unix only (the scripts run `sh`).
//!
//! The scenarios share one test function on purpose: the fake binary is chosen through an
//! environment variable, and setting it is only sound while no other thread runs.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use fleetify::agents::{COPILOT_BINARY_ENV, check, is_checked};

/// A confined agent: writes `inside.txt` when asked, leaves the parent alone. It records its
/// arguments in the workspace, so the test can read the grants it was started with.
const CONFINED: &str = r#"#!/bin/sh
echo "$@" > args.txt
case "$*" in *inside.txt*) echo inside > inside.txt ;; esac
echo DONE
"#;

/// An agent that writes the escape file one level up, as an unconfined CLI would.
const LEAKY: &str = r#"#!/bin/sh
case "$*" in *inside.txt*) echo inside > inside.txt ;; esac
case "$*" in *escape.txt*) echo escaped > ../escape.txt ;; esac
echo DONE
"#;

/// An agent that never writes anything.
const SILENT: &str = "#!/bin/sh\necho DONE\n";

fn fake_binary(dir: &Path, name: &str, script: &str) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn check_agent_copilot_passes_only_when_confined() {
    let base = std::env::temp_dir().join(format!("fleetify-copilot-fake-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    let workdir = base.join("workdir");
    fs::create_dir_all(&workdir).unwrap();
    let bin = base.join("bin");

    // SAFETY: this is the only test that reads or sets this variable in this binary.
    // A confined agent passes, records the check, and was started with a per-path write grant.
    unsafe { std::env::set_var(COPILOT_BINARY_ENV, fake_binary(&bin, "confined", CONFINED)) };
    check(&workdir, "copilot", None, None).expect("a confined agent passes");
    assert!(is_checked(&workdir, "copilot", None, None));
    let args =
        fs::read_to_string(workdir.join("agents/canary/copilot/workspace/args.txt")).unwrap();
    assert!(args.starts_with("-p "), "{args}");
    assert!(args.contains("--no-ask-user"), "{args}");
    assert!(args.contains("--allow-tool=write("), "{args}");
    assert!(
        !args.contains("--allow-tool=write "),
        "bare write in {args}"
    );

    // An agent that writes the escape file fails, and the pass marker is removed.
    unsafe { std::env::set_var(COPILOT_BINARY_ENV, fake_binary(&bin, "leaky", LEAKY)) };
    let err = check(&workdir, "copilot", None, None).unwrap_err();
    assert!(err.contains("not confined"), "{err}");
    assert!(!is_checked(&workdir, "copilot", None, None));

    // An agent that never writes inside its workspace fails.
    unsafe { std::env::set_var(COPILOT_BINARY_ENV, fake_binary(&bin, "silent", SILENT)) };
    let err = check(&workdir, "copilot", None, None).unwrap_err();
    assert!(err.contains("did not write inside"), "{err}");

    unsafe { std::env::remove_var(COPILOT_BINARY_ENV) };
    let _ = fs::remove_dir_all(&base);
}
