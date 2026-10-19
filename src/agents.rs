//! Coding agent adapters (milestone M9). Only `claude-code` runs in this build;
//! the other names in `recipe::KNOWN_AGENTS` are refused with a clear message.
//!
//! An agent run gets the recipe's instructions in `.agent/task.md`, its attachments
//! in `.agent/attachments/`, a tool allowlist and a timeout. It works in the
//! prepared workspace. The verifier and the guards judge what it changed, as for
//! every other step. `check-agent` is a canary: the agent must write inside its
//! workspace and must not write outside it.

use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use crate::clock::rfc3339_now;
use crate::config::Permissions;
use crate::events::EventBus;
use crate::events::Redactor;
use crate::recipe::Recipe;
use crate::runner::{BASE_ENV, EnvPolicy, RunRequest, Runner};

pub const CLAUDE_CODE: &str = "claude-code";
/// Replaces the `claude` binary. Used by tests and by installs outside PATH.
pub const BINARY_ENV: &str = "FLEETIFY_CLAUDE_BIN";
/// Directory in the workspace for the task and attachments. Kept out of commits.
pub const AGENT_DIR: &str = ".agent";
/// The file an agent writes when it cannot finish. The repo then ends as `needs_input`.
pub const BLOCKED_FILE: &str = "blocked.md";
const TASK_FILE: &str = "task.md";
const ATTACHMENTS_DIR: &str = "attachments";
const PROMPT: &str = "Read .agent/task.md and carry out the task it describes in this repository. Follow its rules exactly.";
const CANARY_PROMPT: &str = "Create a file named inside.txt in your working directory containing the word inside. \
Then create a file named escape.txt in the parent directory of your working directory containing the word escaped. \
If a tool refuses, do not try another way. Then stop.";
/// Tools every agent gets. The agent CLI limits file access to the workspace.
const BASELINE_TOOLS: &[&str] = &["Read", "Edit", "Write", "Glob", "Grep"];
/// Variables an agent may inherit besides [`BASE_ENV`]: its credentials and config location.
const AGENT_ENV: &[&str] = &["ANTHROPIC_API_KEY", "CLAUDE_CONFIG_DIR"];
const CANARY_TIMEOUT: Duration = Duration::from_secs(300);
/// Where `check-agent` keeps its scratch folders and pass markers, below the workdir.
const CHECK_DIR: &str = "agents";

/// Refuses agent names this build cannot run.
pub fn check_supported(name: &str) -> Result<(), String> {
    match name {
        CLAUDE_CODE => Ok(()),
        other => Err(format!(
            "agent {other:?} is not supported in this build; only {CLAUDE_CODE} runs"
        )),
    }
}

/// The environment an agent gets: the base allowlist plus its own variables.
pub fn env_policy() -> EnvPolicy {
    EnvPolicy::Allowlist {
        names: BASE_ENV
            .iter()
            .chain(AGENT_ENV)
            .map(|name| name.to_string())
            .collect(),
        set: Vec::new(),
    }
}

/// The concatenated instruction texts, in recipe order.
pub fn task_text(recipe_dir: &Path, recipe: &Recipe) -> io::Result<String> {
    let mut parts = Vec::new();
    for instruction in &recipe.instructions {
        match (&instruction.file, &instruction.text) {
            (Some(file), _) => parts.push(fs::read_to_string(recipe_dir.join(file))?),
            (None, Some(text)) => parts.push(text.clone()),
            (None, None) => {}
        }
    }
    Ok(parts.join("\n\n"))
}

/// Writes the task file and copies the attachments into `<workspace>/.agent/`.
pub fn write_task(workspace: &Path, recipe_dir: &Path, recipe: &Recipe) -> io::Result<()> {
    let attachments = workspace.join(AGENT_DIR).join(ATTACHMENTS_DIR);
    fs::create_dir_all(&attachments)?;
    fs::write(
        workspace.join(AGENT_DIR).join(TASK_FILE),
        task_text(recipe_dir, recipe)?,
    )?;
    for relative in &recipe.attachments {
        let source = recipe_dir.join(relative);
        if let Some(name) = source.file_name() {
            fs::copy(&source, attachments.join(name))?;
        }
    }
    Ok(())
}

/// What the recipe needs that `config.toml` does not grant. Empty when all is granted.
pub fn missing_permissions(recipe: &Recipe, permissions: &Permissions) -> Vec<String> {
    let mut missing = Vec::new();
    for command in &recipe.needs.commands {
        if !permissions.commands.contains(command) {
            missing.push(format!("command {command}"));
        }
    }
    if recipe.needs.web && !permissions.web {
        missing.push("web".to_string());
    }
    for directory in &recipe.needs.extra_directories {
        if !permissions.extra_directories.contains(directory) {
            missing.push(format!("directory {directory}"));
        }
    }
    missing
}

/// The tools an agent may use: the baseline plus the granted commands and web access.
pub fn allowed_tools(recipe: &Recipe, permissions: &Permissions) -> Vec<String> {
    let mut tools: Vec<String> = BASELINE_TOOLS.iter().map(|tool| tool.to_string()).collect();
    for command in &recipe.needs.commands {
        if permissions.commands.contains(command) {
            tools.push(format!("Bash({command}:*)"));
        }
    }
    if recipe.needs.web && permissions.web {
        tools.extend(["WebFetch", "WebSearch"].map(String::from));
    }
    tools
}

/// The full command line for one agent run of a recipe.
pub fn command(
    name: &str,
    recipe: &Recipe,
    permissions: &Permissions,
) -> Result<Vec<String>, String> {
    check_supported(name)?;
    let unrestricted = permissions.agent.get(name).is_some_and(|p| p.unrestricted);
    Ok(claude_code_argv(
        &binary(),
        PROMPT,
        &allowed_tools(recipe, permissions),
        &recipe.needs.extra_directories,
        unrestricted,
    ))
}

/// The command line of the canary: baseline tools only, no extra directories.
fn canary_command(name: &str) -> Result<Vec<String>, String> {
    check_supported(name)?;
    let tools: Vec<String> = BASELINE_TOOLS.iter().map(|tool| tool.to_string()).collect();
    Ok(claude_code_argv(
        &binary(),
        CANARY_PROMPT,
        &tools,
        &[],
        false,
    ))
}

fn binary() -> String {
    std::env::var(BINARY_ENV).unwrap_or_else(|_| "claude".to_string())
}

/// `claude -p <prompt> [--dangerously-skip-permissions] [--add-dir <dir>]... --allowedTools <tool>...`
/// `--allowedTools` comes last because it takes any number of values.
fn claude_code_argv(
    binary: &str,
    prompt: &str,
    allowed: &[String],
    directories: &[String],
    unrestricted: bool,
) -> Vec<String> {
    let mut argv = vec![binary.to_string(), "-p".to_string(), prompt.to_string()];
    if unrestricted {
        argv.push("--dangerously-skip-permissions".to_string());
    }
    for directory in directories {
        argv.extend(["--add-dir".to_string(), directory.clone()]);
    }
    argv.push("--allowedTools".to_string());
    argv.extend(allowed.iter().cloned());
    argv
}

fn checked_marker(workdir: &Path, name: &str) -> std::path::PathBuf {
    workdir.join(CHECK_DIR).join(format!("{name}.checked"))
}

/// True when `check-agent` passed for this agent and the marker is still there.
pub fn is_checked(workdir: &Path, name: &str) -> bool {
    checked_marker(workdir, name).is_file()
}

/// The canary. It passes when the agent wrote `inside.txt` in its workspace and
/// `escape.txt` did not appear one level above. A pass writes the marker that
/// `require_checked_agent` looks for.
pub fn check(workdir: &Path, name: &str) -> Result<(), String> {
    let argv = canary_command(name)?;
    let root = workdir.join(CHECK_DIR).join("canary").join(name);
    let _ = fs::remove_dir_all(&root);
    let workspace = root.join("workspace");
    fs::create_dir_all(&workspace).map_err(|err| format!("cannot create canary folder: {err}"))?;
    let _ = fs::remove_file(checked_marker(workdir, name));

    let runner = Runner::new(EventBus::new(), Redactor::default());
    let mut request = RunRequest::new(name, argv);
    request.cwd = Some(workspace.clone());
    request.timeout = Some(CANARY_TIMEOUT);
    request.env = env_policy();
    let result = runner.run(request);

    if root.join("escape.txt").exists() {
        return Err("the agent wrote outside its workspace: it is not confined".to_string());
    }
    if !workspace.join("inside.txt").exists() {
        return Err(format!(
            "the agent did not write inside its workspace (outcome: {:?})",
            result.outcome
        ));
    }
    let marker = checked_marker(workdir, name);
    if let Some(parent) = marker.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("cannot record the check: {err}"))?;
    }
    fs::write(&marker, rfc3339_now()).map_err(|err| format!("cannot record the check: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AgentPermissions;
    use std::collections::BTreeMap;

    fn recipe(needs: &str) -> Recipe {
        Recipe::parse(&format!(
            r#"schema = 1
name = "upgrade"
branch = "chore/upgrade"
pr_title = "chore: upgrade"
agent = "claude-code"

[[instructions]]
text = "Do the thing."

[needs]
{needs}
"#
        ))
        .unwrap()
    }

    fn permissions(commands: &[&str], web: bool) -> Permissions {
        Permissions {
            commands: commands.iter().map(|c| c.to_string()).collect(),
            web,
            extra_directories: Vec::new(),
            agent: BTreeMap::new(),
        }
    }

    #[test]
    fn only_claude_code_is_supported() {
        assert!(check_supported("claude-code").is_ok());
        let err = check_supported("opencode").unwrap_err();
        assert!(err.contains("not supported"), "{err}");
    }

    #[test]
    fn granted_commands_become_bash_tools_and_nothing_else() {
        let recipe = recipe("commands = [\"cargo\", \"npm\"]");
        let tools = allowed_tools(&recipe, &permissions(&["cargo"], false));
        assert!(tools.contains(&"Bash(cargo:*)".to_string()));
        assert!(!tools.iter().any(|t| t.contains("npm")));
        assert!(!tools.contains(&"WebFetch".to_string()));
    }

    #[test]
    fn web_needs_a_web_grant() {
        let recipe = recipe("web = true");
        assert!(allowed_tools(&recipe, &permissions(&[], true)).contains(&"WebSearch".to_string()));
        let tools = allowed_tools(&recipe, &permissions(&[], false));
        assert!(!tools.contains(&"WebSearch".to_string()));
    }

    #[test]
    fn missing_grants_are_listed() {
        let recipe = recipe("commands = [\"cargo\"]\nweb = true");
        let missing = missing_permissions(&recipe, &permissions(&[], false));
        assert_eq!(
            missing,
            vec!["command cargo".to_string(), "web".to_string()]
        );
        assert!(missing_permissions(&recipe, &permissions(&["cargo"], true)).is_empty());
    }

    #[test]
    fn argv_puts_the_prompt_first_and_the_tools_last() {
        let argv = claude_code_argv(
            "claude",
            "do it",
            &["Read".to_string(), "Bash(cargo:*)".to_string()],
            &["/data".to_string()],
            false,
        );
        assert_eq!(
            argv,
            [
                "claude",
                "-p",
                "do it",
                "--add-dir",
                "/data",
                "--allowedTools",
                "Read",
                "Bash(cargo:*)"
            ]
        );
    }

    #[test]
    fn unrestricted_adds_the_blanket_flag_only_when_configured() {
        let recipe = recipe("");
        let mut perms = permissions(&[], false);
        let flag = "--dangerously-skip-permissions";
        assert!(
            !command("claude-code", &recipe, &perms)
                .unwrap()
                .contains(&flag.to_string())
        );
        perms.agent.insert(
            "claude-code".to_string(),
            AgentPermissions { unrestricted: true },
        );
        assert!(
            command("claude-code", &recipe, &perms)
                .unwrap()
                .contains(&flag.to_string())
        );
    }

    #[test]
    fn task_text_joins_inline_and_file_instructions() {
        let dir = std::env::temp_dir().join(format!("fleetify-task-{}", std::process::id()));
        fs::create_dir_all(dir.join("prompts")).unwrap();
        fs::write(dir.join("prompts/a.md"), "from file").unwrap();
        let recipe = Recipe::parse(
            r#"schema = 1
name = "x"
branch = "b"
pr_title = "t"
agent = "claude-code"

[[instructions]]
file = "prompts/a.md"

[[instructions]]
text = "inline"
"#,
        )
        .unwrap();
        assert_eq!(task_text(&dir, &recipe).unwrap(), "from file\n\ninline");
        let _ = fs::remove_dir_all(&dir);
    }
}
