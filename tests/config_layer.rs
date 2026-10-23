//! The config.toml layer is gone: behaviour comes from the recipe, grants from
//! credentials.toml. Each case runs the real binary and refuses before any repository is
//! touched, so no network is needed.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "fleetify-config-layer-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A recipe with one agent instruction and the given `[needs]` body.
fn write_recipe(dir: &Path, needs: &str) {
    fs::create_dir_all(dir).unwrap();
    fs::write(
        dir.join("recipe.toml"),
        format!(
            r#"schema = 1
name = "config-layer"
branch = "chore/config-layer"
pr_title = "chore: config layer"
agent = "claude-code"

[[instructions]]
text = "Do nothing."

[needs]
{needs}
"#
        ),
    )
    .unwrap();
}

fn fleetify(workdir: &Path, credentials: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_fleetify"));
    command
        .arg("--workdir")
        .arg(workdir)
        .arg("--credentials")
        .arg(credentials);
    command
}

fn run_refused(workdir: &Path, credentials: &Path, recipe: &Path) -> Output {
    fs::create_dir_all(workdir).unwrap();
    let repos = workdir.join("repos.txt");
    fs::write(&repos, "github:owner/repo\n").unwrap();
    fleetify(workdir, credentials)
        .arg("run")
        .arg(recipe)
        .arg(&repos)
        .output()
        .unwrap()
}

#[test]
fn a_config_toml_next_to_the_credentials_refuses_the_run() {
    let base = scratch("leftover-config-dir");
    let recipe = base.join("recipe");
    write_recipe(&recipe, "");
    let config_dir = base.join("config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("config.toml"),
        "[permissions]\ncommands = [\"cargo\"]\n",
    )
    .unwrap();

    let output = run_refused(
        &base.join("work"),
        &config_dir.join("credentials.toml"),
        &recipe,
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(3), "{stderr}");
    assert!(stderr.contains("config.toml is no longer read"), "{stderr}");
    assert!(stderr.contains("credentials.toml"), "{stderr}");
}

#[test]
fn a_config_toml_in_the_working_directory_refuses_the_run() {
    let base = scratch("leftover-config-workdir");
    let recipe = base.join("recipe");
    write_recipe(&recipe, "");
    let workdir = base.join("work");
    fs::create_dir_all(&workdir).unwrap();
    fs::write(workdir.join("config.toml"), "branch_prefix = \"bot/\"\n").unwrap();

    let output = run_refused(&workdir, &base.join("credentials.toml"), &recipe);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(3), "{stderr}");
    assert!(stderr.contains("config.toml is no longer read"), "{stderr}");
}

#[test]
fn a_recipe_asking_for_an_ungranted_command_is_refused_without_a_credentials_file() {
    let base = scratch("ungranted");
    let recipe = base.join("recipe");
    write_recipe(&recipe, "commands = [\"cargo\"]");

    let output = run_refused(&base.join("work"), &base.join("credentials.toml"), &recipe);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(3), "{stderr}");
    assert!(
        stderr.contains("credentials.toml does not grant what the recipe asks for: command cargo"),
        "{stderr}"
    );
}

#[test]
fn a_granted_command_passes_the_grant_check() {
    let base = scratch("granted");
    let recipe = base.join("recipe");
    write_recipe(&recipe, "commands = [\"cargo\"]");
    let credentials = base.join("credentials.toml");
    fs::write(&credentials, "[permissions]\ncommands = [\"cargo\"]\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&credentials, fs::Permissions::from_mode(0o600)).unwrap();
    }

    // The next refusal is the check-agent gate, which comes after the grant check.
    let output = run_refused(&base.join("work"), &credentials, &recipe);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("does not grant"), "{stderr}");
    assert!(stderr.contains("has not passed check-agent"), "{stderr}");
}

#[test]
fn validate_prints_the_effective_settings_and_what_is_not_granted() {
    let base = scratch("validate-settings");
    let recipe = base.join("recipe");
    write_recipe(&recipe, "commands = [\"cargo\"]");

    let output = fleetify(&base.join("work"), &base.join("credentials.toml"))
        .arg("validate")
        .arg(&recipe)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("branch: fleetify/chore/config-layer"),
        "{stdout}"
    );
    assert!(
        stdout.contains("policy.require_checked_agent = true"),
        "{stdout}"
    );
    assert!(stdout.contains("ci.dispatch_after_s = 180"), "{stdout}");
    assert!(
        stdout.contains("ci.log_lines = 100, ci.log_chars = 1000"),
        "{stdout}"
    );
    assert!(stdout.contains("rate_limit.max_wait_min = 15"), "{stdout}");
    assert!(
        stdout.contains("grants: NOT granted here: command cargo"),
        "{stdout}"
    );
}
