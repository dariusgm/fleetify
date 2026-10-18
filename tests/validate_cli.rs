//! Runs the real binary against the example recipe and a broken copy of it.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn fleetify() -> Command {
    Command::new(env!("CARGO_BIN_EXE_fleetify"))
}

fn example_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/recipes/enable-gitlab-ci")
}

fn workdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fleetify-cli-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn example_recipe_validates_and_prints_both_oses() {
    let work = workdir("valid");
    let output = fleetify()
        .arg("--workdir")
        .arg(&work)
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
        stdout.contains("recipe enable-gitlab-ci is valid"),
        "{stdout}"
    );
    assert!(stdout.contains("verify [posix]: bash"), "{stdout}");
    assert!(stdout.contains("verify [windows]: pwsh"), "{stdout}");
    let _ = fs::remove_dir_all(&work);
}

#[test]
fn broken_recipe_is_refused_with_exit_code_three_and_named_fields() {
    let work = workdir("broken");
    let recipe_dir = work.join("recipe");
    fs::create_dir_all(&recipe_dir).unwrap();
    for entry in fs::read_dir(example_dir()).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &recipe_dir.join(entry.file_name()));
        } else {
            fs::copy(entry.path(), recipe_dir.join(entry.file_name())).unwrap();
        }
    }
    let toml = fs::read_to_string(recipe_dir.join("recipe.toml")).unwrap();
    let broken = toml
        .replace("schema = 1", "schema = 2")
        .replace("max_lines_changed = 2000", "max_lines_changed = 0")
        .replace("\"{repo}\"]", "\"{reponame}\"]");
    fs::write(recipe_dir.join("recipe.toml"), broken).unwrap();

    let output = fleetify()
        .arg("--workdir")
        .arg(&work)
        .arg("validate")
        .arg(&recipe_dir)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(3), "{stderr}");
    assert!(stderr.contains("error: schema:"), "{stderr}");
    assert!(
        stderr.contains("error: guards.max_lines_changed:"),
        "{stderr}"
    );
    assert!(
        stderr.contains("unknown placeholder {reponame}"),
        "{stderr}"
    );
    assert_eq!(stderr.matches("unknown placeholder").count(), 1, "{stderr}");
    let _ = fs::remove_dir_all(&work);
}

#[test]
fn schema_command_prints_json_schema_with_recipe_fields() {
    let output = fleetify().arg("schema").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"applies_to\""), "{stdout}");
    assert!(stdout.contains("\"pr_title\""), "{stdout}");
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}
