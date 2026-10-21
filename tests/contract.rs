//! The data contract with the Python authoring package (see contract/README.md).

use std::path::{Path, PathBuf};
use std::process::Command;

fn fleetify() -> Command {
    Command::new(env!("CARGO_BIN_EXE_fleetify"))
}

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn the_schema_snapshot_is_the_binary_schema() {
    let output = fleetify().arg("schema").output().unwrap();
    assert!(output.status.success());
    let snapshot = std::fs::read_to_string(root().join("contract/recipe.schema.json")).unwrap();
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim_end(),
        snapshot.trim_end(),
        "contract/recipe.schema.json is stale: regenerate it (see contract/README.md)"
    );
}

#[test]
fn the_golden_recipe_the_python_package_writes_is_valid_for_the_binary() {
    // The golden file is what authoring/examples/enable_gitlab_ci.py must reproduce; here the
    // binary checks it, in place, against the repo's real files.
    let recipe_dir: PathBuf = root().join("examples/recipes/enable-gitlab-ci");
    let output = fleetify()
        .arg("validate")
        .arg(&recipe_dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
