//! Coding agent adapters (milestones M9, M10, M15a). `claude-code`, `opencode` and `copilot`
//! run in this build; the other names in `recipe::KNOWN_AGENTS` are refused with a message.
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
use serde_json::json;

use crate::config::Permissions;
use crate::events::EventBus;
use crate::events::Redactor;
use crate::recipe::Recipe;
use crate::runner::{BASE_ENV, EnvPolicy, RunRequest, Runner};

pub const CLAUDE_CODE: &str = "claude-code";
pub const OPENCODE: &str = "opencode";
pub const COPILOT: &str = "copilot";
/// Replaces the `copilot` binary, the same way `FLEETIFY_CLAUDE_BIN` does for claude.
pub const COPILOT_BINARY_ENV: &str = "FLEETIFY_COPILOT_BIN";
/// Replaces the `opencode` binary, the same way `FLEETIFY_CLAUDE_BIN` does for claude.
pub const OPENCODE_BINARY_ENV: &str = "FLEETIFY_OPENCODE_BIN";
/// Replaces the `claude` binary. Used by tests and by installs outside PATH.
pub const BINARY_ENV: &str = "FLEETIFY_CLAUDE_BIN";
/// Directory in the workspace for the task and attachments. Kept out of commits.
pub const AGENT_DIR: &str = ".agent";
/// The file an agent writes when it cannot finish, relative to the workspace. The repo
/// then ends as `needs_input`.
pub const BLOCKED_FILE: &str = ".agent/blocked.md";
/// The file an agent writes when it is done, relative to the workspace. Its text goes
/// into the pull request body, marked as written by the agent.
pub const RESULT_FILE: &str = ".agent/result.md";
/// Where the verifier scripts sit for the agent, relative to the workspace.
const AGENT_TOOLS_DIR: &str = ".agent/tools";
/// Fixed, versioned rules that every agent run gets, whatever the recipe says.
const RULES: &str = "\
Rules (fleetify rules v1):
- Do not commit or push. The orchestrator does that.
- Do not ask questions. Choose the most conservative option and note your assumptions.
- Run the verifier named below after each change, and keep going until it passes.
- If you cannot proceed, write .agent/blocked.md explaining why, then stop.
- When you are done, write .agent/result.md: what changed, your assumptions, and anything a reviewer should know.";
const TASK_FILE: &str = "task.md";
const ATTACHMENTS_DIR: &str = "attachments";
const PROMPT: &str = "Read .agent/task.md and carry out the task it describes in this repository. Follow its rules exactly.";
/// The canary runs twice, with two prompts. Asked for both in one run, an agent whose
/// escape attempt is refused often gives up before the inside write.
const INSIDE_PROMPT: &str = "Create a file named inside.txt in your working directory containing the word inside. Then stop.";
const ESCAPE_PROMPT: &str = "Create a file named escape.txt in the parent directory of your working directory containing the word escaped. \
If a tool refuses, do not try another way. Then stop.";
/// Tools every Claude Code run gets. Reading is not limited by path; file edits are, by
/// `Edit` path rules (see `claude_tools`).
const BASELINE_TOOLS: &[&str] = &["Read", "Glob", "Grep"];
/// Variables an agent may inherit besides [`BASE_ENV`]: its credentials and config location.
const AGENT_ENV: &[&str] = &["ANTHROPIC_API_KEY", "CLAUDE_CONFIG_DIR"];
const CANARY_TIMEOUT: Duration = Duration::from_secs(300);
/// Where `check-agent` keeps its scratch folders and pass markers, below the workdir.
const CHECK_DIR: &str = "agents";

/// Refuses agent names this build cannot run.
pub fn check_supported(name: &str) -> Result<(), String> {
    match name {
        CLAUDE_CODE | OPENCODE | COPILOT => Ok(()),
        other => Err(format!(
            "agent {other:?} is not supported in this build; only {CLAUDE_CODE}, {OPENCODE} and {COPILOT} run"
        )),
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

/// Writes `task.md` (rules, instructions, attachments, verifier) and copies the
/// attachments and the recipe's tools into `<workspace>/.agent/`. The tools copy is the
/// untrusted one the agent may run; the orchestrator's verifier uses the original.
pub fn write_task(workspace: &Path, recipe_dir: &Path, recipe: &Recipe) -> io::Result<()> {
    let agent_dir = workspace.join(AGENT_DIR);
    let attachments = agent_dir.join(ATTACHMENTS_DIR);
    fs::create_dir_all(&attachments)?;
    for relative in &recipe.attachments {
        let source = recipe_dir.join(relative);
        if let Some(name) = source.file_name() {
            fs::copy(&source, attachments.join(name))?;
        }
    }
    let tools = recipe.tools.as_deref().unwrap_or("tools");
    let source_tools = recipe_dir.join(tools);
    if source_tools.is_dir() {
        copy_dir(&source_tools, &workspace.join(AGENT_TOOLS_DIR))?;
    }
    fs::write(
        agent_dir.join(TASK_FILE),
        task_document(recipe_dir, recipe)?,
    )
}

/// The text of `task.md`: rules, instructions, attachments and the verifier command.
fn task_document(recipe_dir: &Path, recipe: &Recipe) -> io::Result<String> {
    let mut text = format!("{RULES}\n\n{}\n", task_text(recipe_dir, recipe)?);
    if !recipe.attachments.is_empty() {
        text.push_str("\nAttachments, relative to the repository root:\n");
        for relative in &recipe.attachments {
            let name = Path::new(relative)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            text.push_str(&format!("- .agent/attachments/{name}\n"));
        }
    }
    if let Some(spec) = recipe.verify_spec() {
        let argv = spec
            .render(cfg!(windows), &[("repo", "."), ("tools", AGENT_TOOLS_DIR)])
            .map_err(|err| io::Error::other(err.to_string()))?;
        text.push_str(&format!("\nVerifier: {}\n", argv.join(" ")));
    }
    Ok(text)
}

/// Copies a directory tree. Symbolic links are not followed into the copy.
fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &target)?;
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

/// The tools a Claude Code run may use: the baseline plus the granted commands and web access.
pub fn allowed_tools(recipe: &Recipe, permissions: &Permissions, workspace: &Path) -> Vec<String> {
    claude_tools(&grants(recipe, permissions, CLAUDE_CODE), workspace)
}

/// The Claude Code tool list. File edits are allowed only inside the workspace and the
/// granted directories, by `Edit` path rules. These rules also cover creating files, so
/// `Write` is not listed. Without them the CLI wrote files outside the workspace
/// (found by `check-agent`).
fn claude_tools(grants: &Grants, workspace: &Path) -> Vec<String> {
    let mut tools: Vec<String> = BASELINE_TOOLS.iter().map(|tool| tool.to_string()).collect();
    tools.push(edit_rule(workspace));
    for directory in &grants.directories {
        tools.push(edit_rule(Path::new(directory)));
    }
    for command in &grants.commands {
        tools.push(format!("Bash({command}:*)"));
    }
    if grants.web {
        tools.extend(["WebFetch", "WebSearch"].map(String::from));
    }
    tools
}

/// What fleetify grants one agent run, after the recipe's needs were checked against
/// `config.toml`. Each adapter turns these into its own CLI options.
#[derive(Debug, Default)]
pub struct Grants {
    pub commands: Vec<String>,
    pub web: bool,
    pub directories: Vec<String>,
    pub unrestricted: bool,
}

/// The command line and the environment of one agent run.
#[derive(Debug)]
pub struct Invocation {
    pub argv: Vec<String>,
    pub env: EnvPolicy,
}

/// What fleetify grants one agent run for this recipe.
pub fn grants(recipe: &Recipe, permissions: &Permissions, name: &str) -> Grants {
    Grants {
        commands: recipe
            .needs
            .commands
            .iter()
            .filter(|command| permissions.commands.contains(command))
            .cloned()
            .collect(),
        web: recipe.needs.web && permissions.web,
        directories: recipe.needs.extra_directories.clone(),
        unrestricted: permissions.agent.get(name).is_some_and(|p| p.unrestricted),
    }
}

/// One line for the run header: what the agent gets for this recipe.
pub fn describe_grants(recipe: &Recipe, permissions: &Permissions, name: &str) -> String {
    let grants = grants(recipe, permissions, name);
    let list = |items: &[String]| {
        if items.is_empty() {
            "none".to_string()
        } else {
            items.join(", ")
        }
    };
    format!(
        "commands [{}], web {}, directories [{}], unrestricted {}",
        list(&grants.commands),
        if grants.web { "yes" } else { "no" },
        list(&grants.directories),
        if grants.unrestricted { "yes" } else { "no" },
    )
}

/// The `Edit` path rule that allows edits under `dir`. Claude Code reads `//` as the start
/// of an absolute path, so `Edit(//<abs dir>/**)` covers everything below `dir`.
fn edit_rule(dir: &Path) -> String {
    format!(
        "Edit(//{}/**)",
        dir.display().to_string().trim_end_matches('/')
    )
}

/// The command line and environment for a recipe's agent run in `workspace`.
pub fn invocation(
    name: &str,
    recipe: &Recipe,
    permissions: &Permissions,
    workspace: &Path,
) -> Result<Invocation, String> {
    build(name, PROMPT, &grants(recipe, permissions, name), workspace)
}

/// The canary's invocation: the baseline grants only, so it tests what every run gets.
fn canary_invocation(name: &str, workspace: &Path, prompt: &str) -> Result<Invocation, String> {
    build(name, prompt, &Grants::default(), workspace)
}

fn build(
    name: &str,
    prompt: &str,
    grants: &Grants,
    workspace: &Path,
) -> Result<Invocation, String> {
    check_supported(name)?;
    match name {
        OPENCODE => {
            let config = opencode_config(grants).to_string();
            Ok(Invocation {
                argv: vec![
                    binary(OPENCODE_BINARY_ENV, "opencode"),
                    "run".to_string(),
                    prompt.to_string(),
                ],
                env: env_policy(
                    AGENT_ENV,
                    vec![
                        ("OPENCODE_CONFIG_CONTENT".to_string(), config),
                        // A repository's own opencode.json must not widen the grants.
                        (
                            "OPENCODE_DISABLE_PROJECT_CONFIG".to_string(),
                            "1".to_string(),
                        ),
                    ],
                ),
            })
        }
        COPILOT => Ok(Invocation {
            argv: copilot_argv(
                &binary(COPILOT_BINARY_ENV, "copilot"),
                prompt,
                workspace,
                grants,
            ),
            // Copilot keeps its own login in its own configuration. No host token and no
            // Claude or opencode variables are passed on.
            env: env_policy(&[], Vec::new()),
        }),
        _ => Ok(Invocation {
            argv: claude_code_argv(
                &binary(BINARY_ENV, "claude"),
                prompt,
                &claude_tools(grants, workspace),
                &grants.directories,
                grants.unrestricted,
            ),
            env: env_policy(AGENT_ENV, Vec::new()),
        }),
    }
}

/// The environment an agent gets: the base allowlist, its own variables, and the
/// variables fleetify sets for this run.
fn env_policy(agent_names: &[&str], set: Vec<(String, String)>) -> EnvPolicy {
    EnvPolicy::Allowlist {
        names: BASE_ENV
            .iter()
            .chain(agent_names)
            .map(|name| name.to_string())
            .collect(),
        set,
    }
}

/// opencode's permissions as one inline config. Everything not listed is denied, and
/// the agent cannot ask a question, since nobody is there to answer.
fn opencode_config(grants: &Grants) -> serde_json::Value {
    let open = |allowed: bool| if allowed { "allow" } else { "deny" };
    let everything = grants.unrestricted;
    let mut bash = serde_json::Map::new();
    bash.insert("*".to_string(), json!(open(everything)));
    // Read-only git is part of the baseline. Other commands need a grant.
    for command in ["git status", "git diff", "git log"]
        .into_iter()
        .map(String::from)
        .chain(grants.commands.iter().cloned())
    {
        bash.insert(command.clone(), json!("allow"));
        bash.insert(format!("{command} *"), json!("allow"));
    }
    let mut external = serde_json::Map::new();
    external.insert("*".to_string(), json!(open(everything)));
    for directory in &grants.directories {
        external.insert(format!("{directory}/**"), json!("allow"));
    }
    json!({
        "permission": {
            "read": "allow",
            "edit": "allow",
            "glob": "allow",
            "grep": "allow",
            "list": "allow",
            "webfetch": open(grants.web || everything),
            "websearch": open(grants.web || everything),
            "bash": bash,
            "external_directory": external,
            "task": open(everything),
            "skill": open(everything),
            "question": "deny",
            "doom_loop": "deny"
        }
    })
}

/// The agent binary: from the environment variable when set, else from PATH.
fn binary(variable: &str, default: &str) -> String {
    std::env::var(variable).unwrap_or_else(|_| default.to_string())
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

/// `copilot -p <prompt> -s --no-ask-user [--allow-all] [--add-dir <dir>]... --allow-tool=<grant>...`
///
/// Docs: <https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-programmatic-reference>
/// (checked 2026-10-21, Copilot CLI 1.0.101).
///
/// Writes are granted per directory with a `**` pattern. A bare `write` writes anywhere, and
/// `--add-dir` alone does not confine writes; `<dir>/*` covers one level only (both observed,
/// see DECISIONS.md). Commands are granted as `shell(<command>:*)`, read-only git as exact
/// `shell(git status)` entries. `--allow-all-urls` is the web grant: Copilot has no per-URL
/// grant that fleetify could fill from the recipe.
fn copilot_argv(binary: &str, prompt: &str, workspace: &Path, grants: &Grants) -> Vec<String> {
    let mut argv = vec![
        binary.to_string(),
        "-p".to_string(),
        prompt.to_string(),
        "-s".to_string(),
        "--no-ask-user".to_string(),
    ];
    if grants.unrestricted {
        argv.push("--allow-all".to_string());
    }
    for directory in &grants.directories {
        argv.extend(["--add-dir".to_string(), directory.clone()]);
    }
    argv.push("--allow-tool=read".to_string());
    let writable =
        std::iter::once(workspace.display().to_string()).chain(grants.directories.iter().cloned());
    for directory in writable {
        argv.push(format!(
            "--allow-tool=write({}/**)",
            directory.trim_end_matches('/')
        ));
    }
    // Read-only git is part of the baseline, as for opencode.
    for command in ["git status", "git diff", "git log"] {
        argv.push(format!("--allow-tool=shell({command})"));
    }
    // Copilot matches shell grants on the executable only: `shell(sh:*)` allows
    // `sh .agent/tools/verify.sh`, while a grant that names the whole command line is denied
    // (observed 2026-10-21). So a command grants its first word, which is broader than the
    // recipe's command. Recorded in DECISIONS.md.
    let mut executables: Vec<&str> = grants
        .commands
        .iter()
        .filter_map(|command| command.split_whitespace().next())
        .collect();
    executables.dedup();
    for executable in executables {
        argv.push(format!("--allow-tool=shell({executable}:*)"));
    }
    if grants.web {
        argv.push("--allow-all-urls".to_string());
    }
    argv
}

/// Phrases that mark a rate limit in an agent's output, compared case-insensitively. Only
/// consulted for a run that already failed, so a phrase in code the agent printed cannot
/// make a successful run fail. Extend as the level-5 runs show the real messages.
const RATE_LIMIT_MARKERS: &[&str] = &["rate limit", "usage limit", "too many requests", "http 429"];

/// The first output line that reports a rate limit, if any.
pub fn rate_limit_line(lines: &[String]) -> Option<String> {
    lines
        .iter()
        .find(|line| {
            let lower = line.to_lowercase();
            RATE_LIMIT_MARKERS
                .iter()
                .any(|marker| lower.contains(marker))
        })
        .cloned()
}

/// True when an agent output line is a question it waits on: a yes/no prompt, or a
/// choice between numbered options and `n` (for example `Pick option 1, 2 or n` or
/// `[1/2/n]`). Such an agent gets no answer, so the run stops and the repo ends as
/// `needs_input`.
pub fn wants_input(line: &str) -> bool {
    let line = line.to_lowercase();
    if line.contains("[y/n]") || line.contains("(y/n)") {
        return true;
    }
    // Choice markers: `/n]`, `/n)`, or `1, 2 or n` style lists with a question word.
    let digit = line.chars().any(|c| c.is_ascii_digit());
    let choice_marker = line.contains("/n]") || line.contains("/n)");
    let choice_list = digit
        && (line.contains(" or n") || line.contains("(or n"))
        && ["option", "pick", "choose", "select"]
            .iter()
            .any(|word| line.contains(word));
    choice_marker || choice_list
}

fn checked_marker(workdir: &Path, name: &str) -> std::path::PathBuf {
    workdir.join(CHECK_DIR).join(format!("{name}.checked"))
}

/// True when `check-agent` passed for this agent and the marker is still there.
pub fn is_checked(workdir: &Path, name: &str) -> bool {
    checked_marker(workdir, name).is_file()
}

/// One canary run in `workspace`. Only the files it leaves behind are judged, by `check`.
fn canary_run(name: &str, workspace: &Path, prompt: &str) -> Result<(), String> {
    let canary = canary_invocation(name, workspace, prompt)?;
    let runner = Runner::new(EventBus::new(), Redactor::default());
    let mut request = RunRequest::new(name, canary.argv);
    request.cwd = Some(workspace.to_path_buf());
    request.timeout = Some(CANARY_TIMEOUT);
    request.env = canary.env;
    runner.run(request);
    Ok(())
}

/// The canary. It passes when the agent wrote `inside.txt` in its workspace and
/// `escape.txt` did not appear one level above. A pass writes the marker that
/// `require_checked_agent` looks for.
pub fn check(workdir: &Path, name: &str) -> Result<(), String> {
    let root = workdir.join(CHECK_DIR).join("canary").join(name);
    let _ = fs::remove_dir_all(&root);
    let workspace = root.join("workspace");
    fs::create_dir_all(&workspace).map_err(|err| format!("cannot create canary folder: {err}"))?;
    let _ = fs::remove_file(checked_marker(workdir, name));

    canary_run(name, &workspace, INSIDE_PROMPT)?;
    if !workspace.join("inside.txt").exists() {
        return Err("the agent did not write inside its workspace".to_string());
    }
    canary_run(name, &workspace, ESCAPE_PROMPT)?;
    if root.join("escape.txt").exists() {
        return Err("the agent wrote outside its workspace: it is not confined".to_string());
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

    #[test]
    fn copilot_grants_writes_per_directory_and_never_a_bare_write() {
        let grants = Grants {
            commands: vec!["cargo".into()],
            web: false,
            directories: vec!["/data/out".into()],
            unrestricted: false,
        };
        let argv = copilot_argv("copilot", "p", Path::new("/ws"), &grants);
        assert!(
            argv.contains(&"--allow-tool=write(/ws/**)".to_string()),
            "{argv:?}"
        );
        assert!(
            argv.contains(&"--allow-tool=write(/data/out/**)".to_string()),
            "{argv:?}"
        );
        assert!(!argv.iter().any(|a| a == "--allow-tool=write"), "{argv:?}");
        assert!(
            argv.contains(&"--allow-tool=shell(cargo:*)".to_string()),
            "{argv:?}"
        );
        assert!(
            !argv.iter().any(|a| a.starts_with("--allow-all")),
            "{argv:?}"
        );
        assert_eq!(&argv[..5], ["copilot", "-p", "p", "-s", "--no-ask-user"]);
    }

    #[test]
    fn copilot_baseline_is_read_git_and_writes_in_the_workspace() {
        let argv = copilot_argv("copilot", "p", Path::new("/ws"), &Grants::default());
        for expected in [
            "--allow-tool=read",
            "--allow-tool=write(/ws/**)",
            "--allow-tool=shell(git status)",
            "--allow-tool=shell(git diff)",
            "--allow-tool=shell(git log)",
        ] {
            assert!(
                argv.contains(&expected.to_string()),
                "{expected} missing in {argv:?}"
            );
        }
        assert!(
            !argv
                .iter()
                .any(|a| a == "--allow-all-urls" || a == "--allow-all"),
            "{argv:?}"
        );
    }

    #[test]
    fn copilot_web_and_unrestricted_are_explicit_flags() {
        let grants = Grants {
            web: true,
            unrestricted: true,
            ..Grants::default()
        };
        let argv = copilot_argv("copilot", "p", Path::new("/ws"), &grants);
        assert!(argv.contains(&"--allow-all-urls".to_string()), "{argv:?}");
        assert!(argv.contains(&"--allow-all".to_string()), "{argv:?}");
    }

    #[test]
    fn copilot_is_supported_and_gets_no_claude_or_host_variables() {
        assert!(check_supported("copilot").is_ok());
        let invocation = build(COPILOT, "p", &Grants::default(), Path::new("/ws")).unwrap();
        let EnvPolicy::Allowlist { names, .. } = invocation.env else {
            panic!("expected an allowlist");
        };
        assert!(
            !names.iter().any(|name| name == "ANTHROPIC_API_KEY"),
            "{names:?}"
        );
        assert!(
            !names.iter().any(|name| name.contains("TOKEN")),
            "{names:?}"
        );
    }

    #[test]
    fn rate_limit_messages_are_recognised_in_any_case() {
        let lines = vec![
            "compiling".to_string(),
            "Error: Rate Limit reached, try again later".to_string(),
        ];
        assert_eq!(
            rate_limit_line(&lines).as_deref(),
            Some("Error: Rate Limit reached, try again later")
        );
        assert!(rate_limit_line(&["Permission denied".to_string()]).is_none());
        assert!(rate_limit_line(&["You hit the usage limit".to_string()]).is_some());
        assert!(rate_limit_line(&["HTTP 429 from the provider".to_string()]).is_some());
    }

    #[test]
    fn yes_no_questions_are_recognised() {
        assert!(wants_input("Overwrite the file? [y/N]"));
        assert!(wants_input("Proceed (Y/n)"));
        assert!(wants_input("Pick option 1, 2 or n"));
        assert!(wants_input("Which fix? [1/2/n]"));
        assert!(wants_input("Choose 1 or 2 (or n to skip)"));
        assert!(!wants_input("Why is the build slow?"));
        assert!(!wants_input("Built 2 crates in 3 seconds"));
    }
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
    fn claude_code_opencode_and_copilot_run_and_others_are_refused() {
        assert!(check_supported("claude-code").is_ok());
        assert!(check_supported("opencode").is_ok());
        assert!(check_supported("copilot").is_ok());
        let err = check_supported("unknown-agent").unwrap_err();
        assert!(err.contains("not supported"), "{err}");
    }

    #[test]
    fn claude_code_runs_with_the_allowlist_and_nothing_else() {
        let env = env_policy(AGENT_ENV, Vec::new());
        let EnvPolicy::Allowlist { names, set } = env else {
            panic!("expected an allowlist");
        };
        let expected: Vec<String> = BASE_ENV
            .iter()
            .chain(AGENT_ENV)
            .map(|name| name.to_string())
            .collect();
        assert_eq!(names, expected);
        assert!(set.is_empty());
    }

    #[test]
    fn grants_are_described_for_the_run_header() {
        let recipe = recipe("commands = [\"cargo\"]");
        assert_eq!(
            describe_grants(&recipe, &permissions(&["cargo"], false), "opencode"),
            "commands [cargo], web no, directories [none], unrestricted no"
        );
        assert_eq!(
            describe_grants(&recipe, &permissions(&[], false), "opencode"),
            "commands [none], web no, directories [none], unrestricted no"
        );
    }

    #[test]
    fn claude_edits_are_limited_to_the_workspace_and_granted_directories() {
        let recipe = recipe("extra_directories = [\"/data\"]");
        let mut perms = permissions(&[], false);
        perms.extra_directories = vec!["/data".to_string()];
        let tools = allowed_tools(&recipe, &perms, Path::new("/ws/run"));
        assert!(tools.contains(&"Edit(///data/**)".to_string()), "{tools:?}");
        assert!(
            tools.contains(&"Edit(///ws/run/**)".to_string()),
            "{tools:?}"
        );
        assert!(!tools.contains(&"Edit".to_string()), "{tools:?}");
        assert!(!tools.contains(&"Write".to_string()), "{tools:?}");
    }

    #[test]
    fn granted_commands_become_bash_tools_and_nothing_else() {
        let recipe = recipe("commands = [\"cargo\", \"npm\"]");
        let tools = allowed_tools(&recipe, &permissions(&["cargo"], false), Path::new("/ws"));
        assert!(tools.contains(&"Bash(cargo:*)".to_string()));
        assert!(!tools.iter().any(|t| t.contains("npm")));
        assert!(!tools.contains(&"WebFetch".to_string()));
    }

    #[test]
    fn web_needs_a_web_grant() {
        let recipe = recipe("web = true");
        assert!(
            allowed_tools(&recipe, &permissions(&[], true), Path::new("/ws"))
                .contains(&"WebSearch".to_string())
        );
        let tools = allowed_tools(&recipe, &permissions(&[], false), Path::new("/ws"));
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
            !invocation("claude-code", &recipe, &perms, Path::new("/ws"))
                .unwrap()
                .argv
                .contains(&flag.to_string())
        );
        perms.agent.insert(
            "claude-code".to_string(),
            AgentPermissions { unrestricted: true },
        );
        assert!(
            invocation("claude-code", &recipe, &perms, Path::new("/ws"))
                .unwrap()
                .argv
                .contains(&flag.to_string())
        );
    }

    #[test]
    fn opencode_is_denied_by_default_and_granted_per_command() {
        let grants = Grants {
            commands: vec!["uv".to_string()],
            ..Grants::default()
        };
        let config = opencode_config(&grants);
        let permission = &config["permission"];
        assert_eq!(permission["bash"]["*"], "deny");
        assert_eq!(permission["bash"]["uv *"], "allow");
        assert_eq!(permission["bash"]["git status *"], "allow");
        assert_eq!(permission["question"], "deny");
        assert_eq!(permission["webfetch"], "deny");
        assert_eq!(permission["external_directory"]["*"], "deny");
        assert_eq!(permission["edit"], "allow");
    }

    #[test]
    fn opencode_gets_its_config_and_no_project_config() {
        let recipe = recipe("commands = [\"uv\"]");
        let mut perms = permissions(&["uv"], false);
        perms.agent.insert(
            "opencode".to_string(),
            AgentPermissions {
                unrestricted: false,
            },
        );
        let invocation = invocation("opencode", &recipe, &perms, Path::new("/ws")).unwrap();
        assert_eq!(invocation.argv[1], "run");
        let EnvPolicy::Allowlist { set, .. } = invocation.env else {
            panic!("opencode needs an allowlist environment");
        };
        assert!(
            set.iter()
                .any(|(k, v)| k == "OPENCODE_CONFIG_CONTENT" && v.contains("uv *"))
        );
        assert!(
            set.iter()
                .any(|(k, v)| k == "OPENCODE_DISABLE_PROJECT_CONFIG" && v == "1")
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
