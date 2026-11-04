//! The `scripted-update` example: it validates, and its script becomes the per-OS commands
//! the README documents (bash on posix, pwsh on windows). No docker, no network.

use std::path::Path;
use std::process::Command;

use fleetify::recipe::Recipe;

fn example_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/recipes/scripted-update")
}

#[test]
fn the_scripted_example_validates_and_names_both_interpreters() {
    let output = Command::new(env!("CARGO_BIN_EXE_fleetify"))
        .arg("validate")
        .arg(example_dir())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("recipe scripted-update is valid"),
        "{stdout}"
    );
    assert!(stdout.contains("prepare [posix]: bash"), "{stdout}");
    assert!(stdout.contains("prepare [windows]: pwsh"), "{stdout}");
}

#[test]
fn the_example_script_becomes_the_documented_commands() {
    let text = std::fs::read_to_string(example_dir().join("recipe.toml")).unwrap();
    let recipe = Recipe::parse(&text).unwrap();
    let table = recipe.prepare_table().unwrap();
    assert_eq!(
        table.posix.unwrap(),
        ["bash", "{tools}/update.sh", "{repo}"]
    );
    let windows = table.windows.unwrap();
    assert_eq!(windows[0], "pwsh");
    assert!(windows.contains(&"{tools}/update.ps1".to_string()));
    assert_eq!(table.timeout_s, Some(600));
}

/// The PowerShell update script, run by pwsh with a stub `uv` first on the PATH. It must pass
/// the repository path on, run inside it, and give back the exit code of `uv`. Skipped when pwsh
/// is not installed. Unix only: the stub is a shell script.
#[cfg(unix)]
#[test]
fn the_powershell_update_script_runs_in_the_repository_and_returns_uvs_exit_code() {
    if Command::new("pwsh").arg("-v").output().is_err() {
        eprintln!("skipped: pwsh is not installed");
        return;
    }
    let base = std::env::temp_dir().join(format!("fleetify-pwsh-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let repo = base.join("repo");
    let stub_dir = base.join("bin");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&stub_dir).unwrap();
    let record = base.join("record.txt");
    // The stub records its arguments and its working directory, then exits with STUB_RC.
    let stub = format!(
        "#!/bin/sh\necho \"$@\" >> {rec}\npwd >> {rec}\nexit \"${{STUB_RC:-0}}\"\n",
        rec = record.display()
    );
    let uv = stub_dir.join("uv");
    std::fs::write(&uv, stub).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&uv, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = format!(
        "{}:{}",
        stub_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let script = example_dir().join("tools/update.ps1");
    let run = |rc: &str| {
        Command::new("pwsh")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&script)
            .arg(&repo)
            .env("PATH", &path)
            .env("STUB_RC", rc)
            .output()
            .unwrap()
    };

    let ok = run("0");
    assert_eq!(
        ok.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    let recorded = std::fs::read_to_string(&record).unwrap();
    assert!(recorded.contains("lock --upgrade"), "{recorded}");
    assert!(
        recorded.lines().any(|line| line.ends_with("repo")),
        "uv ran in the repository: {recorded}"
    );

    let failed = run("3");
    assert_eq!(failed.status.code(), Some(3), "uv's exit code is returned");
    let _ = std::fs::remove_dir_all(&base);
}
