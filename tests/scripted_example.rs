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
