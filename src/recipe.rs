//! Recipe format: `recipe.toml` model, JSON Schema export, validation and the
//! `applies_to` evaluation. A recipe is data; logic lives in `tools/` scripts.
//!
//! Every behaviour setting lives here, with a default where the recipe is silent. A recipe
//! only asks for permissions (`needs`, `policy`, `ci.merge`); the user's grants live in
//! `credentials.toml` (see [`crate::grants`]). So a recipe copied to another machine runs the
//! same way, as long as that user's grants cover what it asks for.
//!
//! `applies_to` semantics (decided in DECISIONS.md, M3): every listed condition
//! must hold (AND). Within `host_type` and `path_glob` any listed value may match
//! (OR). `has_file` needs *all* listed files, `has_file_any` needs *any* of them,
//! and `not_has_file` needs *none* of them to exist.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Component, Path};

use schemars::JsonSchema;
use serde::Deserialize;

use crate::credentials::glob_match;
use crate::hosts::{HostTable, HostType};
use crate::runner::{CommandSpec, SpecError};

/// The only schema version this binary reads.
pub const SCHEMA_VERSION: u32 = 1;
pub const KNOWN_AGENTS: &[&str] = &["claude-code", "opencode", "copilot"];
pub const KNOWN_HOST_TYPES: &[&str] = &["github", "gitlab"];
const KNOWN_CI_PROVIDERS: &[&str] = &["auto", "gitlab-ci"];
const KNOWN_MERGE_MODES: &[&str] = &["off", "when_green"];
const DEFAULT_PREPARE_TIMEOUT_S: u64 = 120;
const DEFAULT_VERIFY_TIMEOUT_S: u64 = 300;
/// `branch_prefix` when the recipe does not set one: branches are `fleetify/<branch>`.
pub const DEFAULT_BRANCH_PREFIX: &str = "fleetify";
/// Unset `ci.dispatch_after_s`: three minutes.
const DEFAULT_DISPATCH_AFTER_S: u64 = 3 * 60;
/// Unset `ci.log_lines` and `ci.log_chars`.
const DEFAULT_LOG_LINES: usize = 100;
const DEFAULT_LOG_CHARS: usize = 1000;
/// Markers that make CI skip a pipeline. Refused in branch names and commit messages.
const SKIP_CI_MARKERS: &[&str] = &[
    "[skip ci]",
    "[ci skip]",
    "[skip-ci]",
    "[no ci]",
    "[skip actions]",
    "[actions skip]",
    "***no_ci***",
    "skip-ci",
    "ci-skip",
];

#[derive(Debug, Clone, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    /// Must be 1.
    pub schema: u32,
    /// Lowercase name, used in the run id.
    pub name: String,
    /// Branch created in each repo, without the prefix.
    pub branch: String,
    /// Prefix for every branch fleetify pushes: `<prefix>/<branch>`. Unset means `fleetify`;
    /// an empty string means no prefix.
    #[serde(default)]
    pub branch_prefix: Option<String>,
    /// The branch the change starts from and the pull request targets. Unset means the
    /// repository's default branch.
    #[serde(default)]
    pub base: Option<String>,
    /// Title of the pull or merge request.
    pub pr_title: String,
    /// Commit message. Defaults to `pr_title`.
    #[serde(default)]
    pub commit_message: Option<String>,
    /// Agent: claude-code, opencode or copilot. Required when `instructions` is non-empty,
    /// unless `--agent` is given.
    #[serde(default)]
    pub agent: Option<String>,
    /// Model for the recipe's agent, passed to the agent CLI unchanged. Overridden by
    /// `--model`; `[agents.<name>] model` is the fallback.
    #[serde(default)]
    pub model: Option<String>,
    /// Container image the agent runs in. Unset means the agent runs on the host. The image
    /// must carry the label `io.fleetify.agent` with the agent's name (see `container.rs`).
    #[serde(default)]
    pub image: Option<String>,
    /// The isolation backend that runs the agent's container. Unset means docker. Needs
    /// `image`; see `isolation.rs` for the backends this build knows.
    #[serde(default)]
    pub isolation: Option<String>,
    /// Environment variables passed into the agent's container, by name. The values come from
    /// the orchestrator's environment when the run starts; the recipe holds only the names.
    /// Needs `image`.
    #[serde(default)]
    pub agent_env: Vec<String>,
    /// Defaults per agent, keyed by agent name. Used when the run's agent has no `model`
    /// from `--model` or from the recipe's own `model`.
    #[serde(default)]
    pub agents: BTreeMap<String, AgentSettings>,
    #[serde(default)]
    pub applies_to: AppliesTo,
    /// Hosts beyond `github.com` and `gitlab.com`, keyed by host name. A repository's host
    /// comes from its URL; this table says which host type that host is.
    #[serde(default)]
    pub hosts: BTreeMap<String, HostEntry>,
    /// Concatenated in order. Empty means no agent: a pure `prepare` run.
    #[serde(default)]
    pub instructions: Vec<Instruction>,
    /// Files copied to `.agent/attachments/`. Paths relative to the recipe directory.
    #[serde(default)]
    pub attachments: Vec<String>,
    /// Directory with verifier and prepare scripts, relative to the recipe directory.
    #[serde(default)]
    pub tools: Option<String>,
    #[serde(default)]
    pub prepare: Option<CommandTable>,
    /// A scripted change without an agent: one script file per OS, run with bash (posix) or
    /// PowerShell (windows) with the repository as its argument. Same as `prepare`, with the
    /// interpreter chosen for the user. Do not set both.
    #[serde(default)]
    pub script: Option<ScriptTable>,
    #[serde(default)]
    pub needs: Needs,
    #[serde(default)]
    pub verify: Option<VerifyTable>,
    #[serde(default)]
    pub guards: Guards,
    #[serde(default)]
    pub ci: Ci,
    #[serde(default)]
    pub delivery: Delivery,
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub rate_limit: RateLimit,
    #[serde(default)]
    pub policy: Policy,
    /// `[network]`: the hosts an agent container may reach through its proxy.
    #[serde(default)]
    pub network: Network,
    /// `[judge]`: a second opinion on the change, after the guards and before anything is
    /// pushed. Unset means no judge. See tasks/m16-llm-judge.md and docs/design/guards.md.
    #[serde(default)]
    pub judge: Option<Judge>,
}

/// `[judge]`: an LLM reviews the diff and answers `pass`, `fail` or `unsure`. `fail` ends the
/// repo as `guard_violation`, `unsure` as `needs_review`; nothing is pushed on either.
/// Every value that matters is stated here: nothing is inherited from a config file.
#[derive(Debug, Clone, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Judge {
    /// `session` (the executor's agent and model), `model` (another model) or `provider`
    /// (a model from another provider). Required when the section is present.
    #[serde(default)]
    pub level: Option<String>,
    /// Changed lines (added plus deleted, lock files excluded). Above this the verdict is
    /// `unsure` without a model call. Required when the section is present.
    #[serde(default)]
    pub max_diff_lines: Option<u32>,
    /// The judge's agent. Required for `model` and `provider`; rejected for `session`.
    #[serde(default)]
    pub agent: Option<String>,
    /// The judge's model in `provider/model` form, passed to the agent unchanged. Required for
    /// `model` and `provider`; rejected for `session`.
    #[serde(default)]
    pub model: Option<String>,
    /// What the change was asked to do. Unset: the recipe's instructions are the task.
    #[serde(default)]
    pub task: Option<String>,
    /// The judge model's context window in tokens, as the agent's configuration states it.
    /// The judge's input must fit in it, after the answer and the call overhead (the lower
    /// bound wins over `max_diff_lines`). Required when the section is present.
    #[serde(default)]
    pub context_tokens: Option<u32>,
    /// Tokens reserved for the answer. Required when the section is present; must be below
    /// `context_tokens`.
    #[serde(default)]
    pub output_tokens: Option<u32>,
    /// Seconds one judge call may run before it is stopped; a stopped call ends as `unsure`.
    /// Unset means the default, 1800. Must be between 1 and 7200.
    #[serde(default)]
    pub timeout_s: Option<u64>,
}

/// The judge call's timeout when `[judge] timeout_s` is unset.
pub const JUDGE_DEFAULT_TIMEOUT_S: u64 = 1800;
/// The largest value `[judge] timeout_s` accepts.
pub const JUDGE_MAX_TIMEOUT_S: u64 = 7200;

/// The levels `[judge] level` accepts.
pub const JUDGE_LEVELS: &[&str] = &["session", "model", "provider"];

/// `[network]`: the hosts an agent container may reach. Needs `image`, and is required when
/// `image` is set, since the agent must reach its model.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Network {
    /// `host:port` entries. The host is a DNS name or an IPv4 address; names are looked up
    /// again when a connection opens. No ranges, wildcards or schemes.
    #[serde(default)]
    pub allow: Vec<String>,
    /// `host:port` entries the proxy rejects without logging or showing them, for traffic the
    /// recipe does not need (a vendor's telemetry). The same entry rules as `allow`; an entry
    /// may not be in both lists.
    #[serde(default)]
    pub ignore: Vec<String>,
}

/// `[agents.<name>]`: settings for one agent, used when the recipe names no model.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AgentSettings {
    /// Model passed to the agent CLI unchanged. Unset leaves the choice to the agent.
    #[serde(default)]
    pub model: Option<String>,
    /// The agent's own configuration, as a table. Opencode only: it is merged into opencode's
    /// inline config. Keep secrets out of it and write `{env:NAME}` where a key goes;
    /// opencode fills it from the environment of the agent.
    #[serde(default)]
    pub config: Option<serde_json::Value>,
}

/// `[hosts."<name>"]`: one host beyond the built-in ones.
#[derive(Debug, Clone, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HostEntry {
    /// github or gitlab: which API and CI integration the host speaks.
    #[serde(rename = "type")]
    pub kind: HostType,
}

/// `[rate_limit]`: how long fleetify waits when a host asks it to hold back.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RateLimit {
    /// The longest wait for one rate limit, in minutes. A longer wait ends the repo as
    /// `rate_limited` and is not taken. `0` means never wait. Unset means 15.
    #[serde(default)]
    pub max_wait_min: Option<u32>,
}

impl RateLimit {
    /// The ceiling as a duration.
    pub fn max_wait(&self) -> std::time::Duration {
        match self.max_wait_min {
            Some(minutes) => std::time::Duration::from_secs(u64::from(minutes) * 60),
            None => crate::hosts::DEFAULT_MAX_RATE_WAIT,
        }
    }
}

/// `[policy]`: safety settings. Each one can be tightened by a recipe; loosening one needs
/// the matching grant in `credentials.toml`.
#[derive(Debug, Clone, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    /// Refuse agents that did not pass `check-agent`. On by default: an agent that was
    /// never checked may write outside its workspace (see DECISIONS.md, confinement).
    /// `false` needs `[permissions] unchecked_agents = true` in credentials.toml.
    #[serde(default = "default_true")]
    pub require_checked_agent: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            require_checked_agent: true,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AppliesTo {
    /// Any of: github, gitlab.
    #[serde(default)]
    pub host_type: Vec<String>,
    /// All of these files must exist in the cache clone.
    #[serde(default)]
    pub has_file: Vec<String>,
    /// At least one of these files must exist.
    #[serde(default)]
    pub has_file_any: Vec<String>,
    /// None of these files may exist.
    #[serde(default)]
    pub not_has_file: Vec<String>,
    /// Any of these globs must match the repo path (`group/sub/name`); `*` matches `/` too.
    #[serde(default)]
    pub path_glob: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Instruction {
    /// Prompt file, relative to the recipe directory. Exactly one of `file` and `text`.
    #[serde(default)]
    pub file: Option<String>,
    /// Inline prompt text.
    #[serde(default)]
    pub text: Option<String>,
}

/// A command table: one argument list for both OSes, or one per OS.
/// `[script]`: the file names of a scripted change, inside the `tools` directory.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ScriptTable {
    /// A file in the `tools` directory, run with `bash`. Give this or `windows`, or both.
    #[serde(default)]
    pub posix: Option<String>,
    /// A file in the `tools` directory, run with `pwsh` (PowerShell 7). Give this or `posix`.
    #[serde(default)]
    pub windows: Option<String>,
    /// Default 120.
    #[serde(default)]
    pub timeout_s: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CommandTable {
    /// Argument list for both OSes.
    #[serde(default)]
    pub argv: Option<Vec<String>>,
    #[serde(default)]
    pub posix: Option<Vec<String>>,
    #[serde(default)]
    pub windows: Option<Vec<String>>,
    /// Default 120.
    #[serde(default)]
    pub timeout_s: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct VerifyTable {
    #[serde(default)]
    pub argv: Option<Vec<String>>,
    #[serde(default)]
    pub posix: Option<Vec<String>>,
    #[serde(default)]
    pub windows: Option<Vec<String>>,
    /// Default 300.
    #[serde(default)]
    pub timeout_s: Option<u64>,
    /// Lines of failing output shown to the agent. Default 40.
    #[serde(default)]
    pub tail_lines: Option<u32>,
    /// Run the verifier on the untouched base before prepare or any agent. A failing
    /// base stops the repo with `base_broken`, so a later failure is known to come
    /// from the change. Default true.
    #[serde(default = "default_true")]
    pub baseline: bool,
    /// Regexes that select the relevant lines on failure.
    #[serde(default)]
    pub error_patterns: Vec<String>,
}

/// What the recipe asks the agent to have. Each item must be granted by the user in
/// `credentials.toml`, or the run is refused before any repo is touched. Asking grants nothing.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Needs {
    /// Extra commands the agent needs, as `[permissions] commands` lists them.
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub web: bool,
    #[serde(default)]
    pub extra_directories: Vec<String>,
    /// The agent CLI's blanket "allow everything" option. Also needs `[permissions.agent.<name>]
    /// unrestricted = true` in credentials.toml. Not confined by fleetify's grants.
    #[serde(default)]
    pub unrestricted: bool,
}

#[derive(Debug, Clone, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Guards {
    #[serde(default)]
    pub deny_paths: Vec<String>,
    /// Regexes that must not appear in added lines.
    #[serde(default)]
    pub deny_added_patterns: Vec<String>,
    #[serde(default = "default_max_files")]
    pub max_files_changed: u32,
    #[serde(default = "default_max_lines")]
    pub max_lines_changed: u32,
    #[serde(default = "yes")]
    pub forbid_test_deletion: bool,
    #[serde(default = "yes")]
    pub forbid_binary_files: bool,
    /// If set, every changed path must match one of these globs.
    #[serde(default)]
    pub allow_paths: Vec<String>,
}

impl Default for Guards {
    fn default() -> Self {
        Guards {
            deny_paths: Vec::new(),
            deny_added_patterns: Vec::new(),
            max_files_changed: default_max_files(),
            max_lines_changed: default_max_lines(),
            forbid_test_deletion: true,
            forbid_binary_files: true,
            allow_paths: Vec::new(),
        }
    }
}

fn default_max_files() -> u32 {
    50
}
fn default_max_lines() -> u32 {
    2000
}
fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Ci {
    /// auto or gitlab-ci.
    #[serde(default = "auto")]
    pub provider: String,
    /// Wait for CI on the pushed commit before reporting success.
    #[serde(default)]
    pub wait: bool,
    #[serde(default = "default_ci_timeout")]
    pub timeout_min: u32,
    /// off or when_green.
    #[serde(default = "off")]
    pub merge: String,
    /// Check GitLab CI and GitHub Actions files before the build: on the base and
    /// after prepare. Only structure is checked. Default true.
    #[serde(default = "default_true")]
    pub validate: bool,
    /// GitHub only: the workflow file fleetify starts with `workflow_dispatch` when the
    /// host starts no CI for the pushed commit, for example `tests.yml`. The file needs a
    /// `workflow_dispatch` trigger. Unset, fleetify cannot start GitHub CI and the repo ends
    /// as `ci_blocked`.
    #[serde(default)]
    pub dispatch_workflow: Option<String>,
    /// How many times the agent gets the CI failure and tries to fix it, on the same branch.
    /// `0` disables the loop. Default 5.
    #[serde(default = "default_fix_rounds")]
    pub fix_rounds: u32,
    /// Seconds to wait for the host's own CI on the pushed commit. If none has started by
    /// then, fleetify starts it: `workflow_dispatch` on GitHub, a pipeline on GitLab. `0`
    /// starts it at once, without asking the host first. Unset means 180.
    #[serde(default)]
    pub dispatch_after_s: Option<u64>,
    /// Lines of each failed CI job's log that reach the agent (the last ones). Unset means 100.
    #[serde(default)]
    pub log_lines: Option<usize>,
    /// Characters of each failed CI job's log that reach the agent. Unset means 1000. The
    /// smaller of `log_lines` and `log_chars` applies.
    #[serde(default)]
    pub log_chars: Option<usize>,
}

fn default_fix_rounds() -> u32 {
    5
}

impl Default for Ci {
    fn default() -> Self {
        Ci {
            provider: auto(),
            wait: false,
            timeout_min: default_ci_timeout(),
            merge: off(),
            validate: true,
            dispatch_workflow: None,
            fix_rounds: default_fix_rounds(),
            dispatch_after_s: None,
            log_lines: None,
            log_chars: None,
        }
    }
}

impl Ci {
    /// The wait before fleetify starts CI itself.
    pub fn dispatch_after(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.dispatch_after_s.unwrap_or(DEFAULT_DISPATCH_AFTER_S))
    }

    /// How much of each failed job's log reaches the agent.
    pub fn log_budget(&self) -> crate::hosts::LogBudget {
        crate::hosts::LogBudget {
            lines: self.log_lines.unwrap_or(DEFAULT_LOG_LINES),
            chars: self.log_chars.unwrap_or(DEFAULT_LOG_CHARS),
        }
    }
}

fn auto() -> String {
    "auto".to_string()
}
fn off() -> String {
    "off".to_string()
}
fn default_ci_timeout() -> u32 {
    30
}

fn default_true() -> bool {
    true
}

/// What happens after the push. Each step is opt-in beyond the one before it, so a
/// private repository can stop at the branch, at the merge request, or go further.
#[derive(Debug, Clone, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Delivery {
    /// Open a pull or merge request for the pushed branch. `false` ends the repo as `pushed`.
    #[serde(default = "default_true")]
    pub open_mr: bool,
    /// Delete the source branch after its change request was merged. Needs `ci.merge = "when_green"`.
    #[serde(default)]
    pub delete_branch: bool,
}

impl Default for Delivery {
    fn default() -> Self {
        Delivery {
            open_mr: true,
            delete_branch: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    #[serde(default = "default_agent_timeout")]
    pub agent_timeout_min: u32,
    #[serde(default = "default_idle_warn")]
    pub idle_warn_min: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            agent_timeout_min: default_agent_timeout(),
            idle_warn_min: default_idle_warn(),
        }
    }
}

fn default_agent_timeout() -> u32 {
    60
}
fn default_idle_warn() -> u32 {
    10
}

/// One problem found in a recipe. `field` is the TOML path, e.g. `verify.timeout_s`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub field: String,
    pub message: String,
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

/// A command as it will run on one OS, with sample placeholder values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedCommand {
    pub step: &'static str,
    pub os: &'static str,
    pub argv: Vec<String>,
}

/// Facts about a repo needed to evaluate `applies_to`.
pub struct RepoFacts<'a> {
    /// `github` or `gitlab`.
    pub host_type: &'a str,
    /// Repo path without the host, e.g. `group/sub/name`.
    pub path: &'a str,
    /// Root of the cache clone at the default branch.
    pub checkout: &'a Path,
}

#[derive(Debug)]
pub enum RecipeError {
    Read { path: String, message: String },
    Parse(String),
}

impl fmt::Display for RecipeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecipeError::Read { path, message } => write!(f, "cannot read {path}: {message}"),
            RecipeError::Parse(message) => write!(f, "invalid recipe.toml: {message}"),
        }
    }
}

impl std::error::Error for RecipeError {}

impl Recipe {
    /// Parses `recipe.toml` text. Unknown keys and wrong types are errors.
    pub fn parse(text: &str) -> Result<Recipe, RecipeError> {
        toml::from_str(text).map_err(|err| RecipeError::Parse(err.to_string()))
    }

    /// Loads `<dir>/recipe.toml`.
    pub fn load(dir: &Path) -> Result<Recipe, RecipeError> {
        let path = dir.join("recipe.toml");
        let text = std::fs::read_to_string(&path).map_err(|err| RecipeError::Read {
            path: path.display().to_string(),
            message: err.to_string(),
        })?;
        Self::parse(&text)
    }

    /// The conditions that do not hold for this repo. Empty means the recipe applies.
    pub fn unmet_conditions(&self, facts: &RepoFacts<'_>) -> Vec<String> {
        let applies = &self.applies_to;
        let mut unmet = Vec::new();
        if !applies.host_type.is_empty() && !applies.host_type.iter().any(|t| t == facts.host_type)
        {
            unmet.push(format!(
                "host_type is {}, recipe needs one of {}",
                facts.host_type,
                applies.host_type.join(", ")
            ));
        }
        if !applies.path_glob.is_empty()
            && !applies.path_glob.iter().any(|g| glob_match(g, facts.path))
        {
            unmet.push(format!(
                "path {} matches none of {}",
                facts.path,
                applies.path_glob.join(", ")
            ));
        }
        for file in &applies.has_file {
            if !facts.checkout.join(file).exists() {
                unmet.push(format!("missing file {file}"));
            }
        }
        if !applies.has_file_any.is_empty()
            && !applies
                .has_file_any
                .iter()
                .any(|f| facts.checkout.join(f).exists())
        {
            unmet.push(format!(
                "none of these files exist: {}",
                applies.has_file_any.join(", ")
            ));
        }
        for file in &applies.not_has_file {
            if facts.checkout.join(file).exists() {
                unmet.push(format!("file {file} exists"));
            }
        }
        unmet
    }

    /// Checks the recipe against its schema rules and the files in `dir`.
    /// `agent_override` is the `--agent` value, if any.
    pub fn validate(&self, dir: &Path, agent_override: Option<&str>) -> Vec<Issue> {
        let mut issues = Vec::new();
        let mut push = |field: &str, message: String| {
            issues.push(Issue {
                field: field.to_string(),
                message,
            })
        };

        if self.schema != SCHEMA_VERSION {
            push(
                "schema",
                format!(
                    "this fleetify reads schema {SCHEMA_VERSION}, recipe has {}",
                    self.schema
                ),
            );
        }
        if !is_recipe_name(&self.name) {
            push(
                "name",
                "use lowercase letters, digits and '-', starting with a letter or digit".into(),
            );
        }
        if !is_valid_branch(&self.branch) {
            push(
                "branch",
                format!("{:?} is not a valid git branch name", self.branch),
            );
        }
        if let Some(base) = &self.base
            && !is_valid_branch(base)
        {
            push("base", format!("{base:?} is not a valid git branch name"));
        }
        if self.pr_title.trim().is_empty() {
            push("pr_title", "must not be empty".into());
        }
        if let Some(message) = &self.commit_message
            && message.trim().is_empty()
        {
            push("commit_message", "must not be empty".into());
        }
        let commit_message = self.commit_message.as_deref().unwrap_or(&self.pr_title);
        for (field, text) in [
            ("branch", self.branch.as_str()),
            ("pr_title", self.pr_title.as_str()),
            ("commit_message", commit_message),
        ] {
            if let Some(marker) = skip_ci_marker(text) {
                push(
                    field,
                    format!("contains the CI skip marker {marker:?}; CI must run"),
                );
            }
        }

        self.validate_agent(agent_override, &mut push);
        if let Some(model) = &self.model
            && let Err(message) = crate::agents::check_model(model)
        {
            push("model", message);
        }
        self.validate_applies_to(&mut push);
        self.validate_files(dir, &mut push);
        self.validate_commands(dir, &mut push);
        self.validate_settings(&mut push);
        self.validate_hosts_and_agents(&mut push);
        self.validate_judge(agent_override, &mut push);
        // The same problem can be found for both OSes; report it once.
        let mut unique: Vec<Issue> = Vec::with_capacity(issues.len());
        for issue in issues {
            if !unique.contains(&issue) {
                unique.push(issue);
            }
        }
        unique
    }

    fn validate_agent(&self, agent_override: Option<&str>, push: &mut impl FnMut(&str, String)) {
        let effective = agent_override.or(self.agent.as_deref());
        if let Some(agent) = self.agent.as_deref()
            && !KNOWN_AGENTS.contains(&agent)
        {
            push(
                "agent",
                format!(
                    "unknown agent {agent:?}; expected one of {}",
                    KNOWN_AGENTS.join(", ")
                ),
            );
        }
        if let Some(agent) = agent_override
            && !KNOWN_AGENTS.contains(&agent)
        {
            push(
                "--agent",
                format!(
                    "unknown agent {agent:?}; expected one of {}",
                    KNOWN_AGENTS.join(", ")
                ),
            );
        }
        if !self.instructions.is_empty() && effective.is_none() {
            push(
                "agent",
                "instructions are set but no agent is: set agent in recipe.toml or pass --agent"
                    .into(),
            );
        }
        if let Some(image) = &self.image {
            if let Err(message) = crate::container::check_name(image) {
                push("image", message);
            }
            if self.instructions.is_empty() {
                push(
                    "image",
                    "image runs the agent, but the recipe has no instructions".into(),
                );
            }
        }
        if let Some(isolation) = &self.isolation {
            if self.image.is_none() {
                push(
                    "isolation",
                    "needs image: only a container has an isolation backend".into(),
                );
            }
            if !crate::isolation::BACKENDS.contains(&isolation.as_str()) {
                push(
                    "isolation",
                    format!(
                        "unknown isolation {isolation:?}; expected one of {}",
                        crate::isolation::BACKENDS.join(", ")
                    ),
                );
            }
        }
        self.validate_agent_env(push);
        self.validate_network(push);
        for (agent, settings) in &self.agents {
            let Some(config) = &settings.config else {
                continue;
            };
            let field = format!("agents.{agent}.config");
            if agent != crate::agents::OPENCODE {
                push(&field, "only opencode takes a config table".into());
            } else if !config.is_object() {
                push(&field, "must be a table".into());
            } else if config.get("permission").is_some() {
                push(
                    &field,
                    "permission is set by fleetify, not by the recipe".into(),
                );
            }
        }
    }

    /// `[judge]`: the level rules (docs/design: tasks/m16-llm-judge.md, "The three levels").
    /// The executor is the recipe's agent, or the `--agent` value when the recipe names none.
    /// A recipe without an agent is a scripted change: there is no executor to compare with.
    fn validate_judge(&self, agent_override: Option<&str>, push: &mut impl FnMut(&str, String)) {
        let Some(judge) = &self.judge else {
            return;
        };
        match judge.max_diff_lines {
            None => push(
                "judge.max_diff_lines",
                "required when [judge] is present".into(),
            ),
            Some(0) => push("judge.max_diff_lines", "must be greater than 0".into()),
            Some(_) => {}
        }
        let Some(level) = judge.level.as_deref() else {
            push(
                "judge.level",
                "required when [judge] is present: session, model or provider".into(),
            );
            return;
        };
        if !JUDGE_LEVELS.contains(&level) {
            push(
                "judge.level",
                format!("{level:?} is not a level: use session, model or provider"),
            );
            return;
        }
        if judge.context_tokens.is_none() {
            push(
                "judge.context_tokens",
                "required when [judge] is present: the judge model's context window".into(),
            );
        }
        if judge.output_tokens.is_none() {
            push(
                "judge.output_tokens",
                "required when [judge] is present: tokens reserved for the answer".into(),
            );
        }
        if judge.context_tokens == Some(0) {
            push("judge.context_tokens", "must be greater than 0".into());
        }
        if let (Some(context), Some(output)) = (judge.context_tokens, judge.output_tokens)
            && (output == 0 || output >= context)
        {
            push(
                "judge.output_tokens",
                format!("must be greater than 0 and below context_tokens ({context})"),
            );
        }
        if let Some(timeout) = judge.timeout_s
            && (timeout == 0 || timeout > JUDGE_MAX_TIMEOUT_S)
        {
            push(
                "judge.timeout_s",
                format!("{timeout} is out of range: use 1 to {JUDGE_MAX_TIMEOUT_S} seconds"),
            );
        }
        if judge.task.is_none() && self.instructions.is_empty() {
            push(
                "judge.task",
                "the judge needs the task: set judge.task, or instructions".into(),
            );
        }
        if let Some(task) = &judge.task
            && task.trim().is_empty()
        {
            push("judge.task", "must not be empty".into());
        }
        let executor = self.agent.as_deref().or(agent_override);
        let executor_model = self.model.as_deref();

        if level == "session" {
            if judge.agent.is_some() {
                push(
                    "judge.agent",
                    "rejected for level session: the judge reuses the executor's agent".into(),
                );
            }
            if judge.model.is_some() {
                push(
                    "judge.model",
                    "rejected for level session: the judge reuses the executor's model".into(),
                );
            }
            if executor.is_none() {
                push(
                    "judge.level",
                    "session reuses the executor's agent and model, and this recipe has no agent"
                        .into(),
                );
            }
            return;
        }

        match judge.agent.as_deref() {
            None => push("judge.agent", format!("required for level {level}")),
            Some(agent) if agent != crate::agents::OPENCODE => push(
                "judge.agent",
                format!(
                    "the judge runs on opencode only for now; {agent} needs its one-shot mode confirmed first (tasks/m16-llm-judge.md, decision 5)"
                ),
            ),
            Some(_) => {}
        }
        let judge_provider = match judge.model.as_deref() {
            None => {
                push("judge.model", format!("required for level {level}"));
                None
            }
            Some(model) => match provider_of(model) {
                Some(provider) => Some(provider),
                None => {
                    push(
                        "judge.model",
                        format!(
                            "{model:?} must be in provider/model form, as opencode names models"
                        ),
                    );
                    None
                }
            },
        };
        // The executor's model and provider decide what `model` and `provider` mean. Without
        // an executor there is nothing to compare with, so both levels pass this check.
        let Some(executor) = executor else {
            return;
        };
        let Some(executor_model) = executor_model else {
            push(
                "model",
                format!(
                    "level {level} needs the executor's model named in the recipe: an agent default cannot be compared"
                ),
            );
            return;
        };
        if level == "model" && judge.model.as_deref() == Some(executor_model) {
            push(
                "judge.model",
                "must differ from the executor's model for level model".into(),
            );
        }
        if level == "provider" {
            match (judge_provider, executor_provider(executor, executor_model)) {
                (Some(judge), Some(executor)) if judge == executor => push(
                    "judge.model",
                    format!(
                        "provider {judge} is the executor's provider: level provider needs another one"
                    ),
                ),
                (_, None) => push(
                    "model",
                    format!(
                        "the provider of {executor} with model {executor_model:?} is not known to fleetify"
                    ),
                ),
                _ => {}
            }
        }
    }

    fn validate_network(&self, push: &mut impl FnMut(&str, String)) {
        let allow = &self.network.allow;
        if !allow.is_empty() && self.image.is_none() {
            push(
                "network",
                "needs image: only a container has a network to restrict".into(),
            );
        }
        if self.image.is_some() && allow.is_empty() {
            push(
                "network",
                "allow lists no host, so the agent could not reach its model; list the model host as host:port"
                    .into(),
            );
        }
        for entry in allow {
            if let Err(message) = crate::egress::parse_entry(entry) {
                push("network.allow", message);
            }
        }
        if !self.network.ignore.is_empty() && self.image.is_none() {
            push(
                "network",
                "ignore needs image: only a container has a network to restrict".into(),
            );
        }
        if let Err(message) = crate::egress::parse_policy(allow, &self.network.ignore) {
            push("network.ignore", message);
        }
    }

    fn validate_agent_env(&self, push: &mut impl FnMut(&str, String)) {
        if !self.agent_env.is_empty() && self.image.is_none() {
            push(
                "agent_env",
                "needs image: only a container gets these variables".into(),
            );
        }
        let mut seen = std::collections::BTreeSet::new();
        for name in &self.agent_env {
            if let Err(message) = crate::container::check_env_name(name) {
                push("agent_env", message);
            } else if !seen.insert(name) {
                push("agent_env", format!("{name} is listed twice"));
            }
        }
        // The Copilot token goes into a Copilot container and nowhere else, and that container
        // needs it.
        let token = crate::agents::COPILOT_TOKEN_ENV;
        let lists_token = self.agent_env.iter().any(|name| name == token);
        if lists_token && self.agent.as_deref() != Some(crate::agents::COPILOT) {
            push(
                "agent_env",
                format!("{token} is for a Copilot container only"),
            );
        }
        if self.agent.as_deref() == Some(crate::agents::COPILOT)
            && self.image.is_some()
            && !lists_token
        {
            push(
                "agent_env",
                format!("a Copilot container needs {token} in agent_env"),
            );
        }
    }

    fn validate_applies_to(&self, push: &mut impl FnMut(&str, String)) {
        for host_type in &self.applies_to.host_type {
            if !KNOWN_HOST_TYPES.contains(&host_type.as_str()) {
                push(
                    "applies_to.host_type",
                    format!("unknown host type {host_type:?}"),
                );
            }
        }
        for (field, files) in [
            ("applies_to.has_file", &self.applies_to.has_file),
            ("applies_to.has_file_any", &self.applies_to.has_file_any),
            ("applies_to.not_has_file", &self.applies_to.not_has_file),
        ] {
            for file in files {
                if !is_safe_relative(file) {
                    push(
                        field,
                        format!("{file:?} must be a relative path inside the repo"),
                    );
                }
            }
        }
        for glob in &self.applies_to.path_glob {
            if glob.trim().is_empty() {
                push(
                    "applies_to.path_glob",
                    "must not contain empty patterns".into(),
                );
            }
        }
    }

    fn validate_files(&self, dir: &Path, push: &mut impl FnMut(&str, String)) {
        for (index, instruction) in self.instructions.iter().enumerate() {
            let field = format!("instructions[{index}]");
            match (&instruction.file, &instruction.text) {
                (Some(_), Some(_)) | (None, None) => {
                    push(&field, "set exactly one of file and text".into())
                }
                (Some(file), None) => check_file(dir, file, &field, push),
                (None, Some(text)) => {
                    if text.trim().is_empty() {
                        push(&field, "text must not be empty".into());
                    }
                }
            }
        }
        for attachment in &self.attachments {
            check_file(dir, attachment, "attachments", push);
        }
        if let Some(tools) = &self.tools {
            if !is_safe_relative(tools) {
                push(
                    "tools",
                    format!("{tools:?} must be a relative path inside the recipe"),
                );
            } else if !dir.join(tools).is_dir() {
                push("tools", format!("directory {tools} does not exist"));
            }
        }
    }

    /// `[script]`: at least one file, each a plain name inside the `tools` directory.
    fn validate_script(
        &self,
        dir: &Path,
        script: &ScriptTable,
        push: &mut impl FnMut(&str, String),
    ) {
        if script.posix.is_none() && script.windows.is_none() {
            push(
                "script",
                "names no file: give posix, windows or both".into(),
            );
        }
        let Some(tools) = &self.tools else {
            push(
                "script",
                "needs tools: the directory that holds the script".into(),
            );
            return;
        };
        for (field, file) in [
            ("script.posix", &script.posix),
            ("script.windows", &script.windows),
        ] {
            if let Some(file) = file {
                check_file(&dir.join(tools), file, field, push);
            }
        }
        if script.timeout_s == Some(0) {
            push("script.timeout_s", "must be greater than 0".into());
        }
    }

    fn validate_commands(&self, dir: &Path, push: &mut impl FnMut(&str, String)) {
        let tools_dir = self
            .tools
            .as_ref()
            .map(|t| dir.join(t).display().to_string());
        let mut uses_tools = false;

        let mut commands: Vec<(&str, Option<CommandSpec>, u64)> = Vec::new();
        if self.prepare.is_some() && self.script.is_some() {
            push(
                "script",
                "set either [prepare] or [script], not both".into(),
            );
        }
        if let Some(script) = &self.script {
            self.validate_script(dir, script, push);
        }
        if let Some(prepare) = self.prepare_table() {
            let spec = command_spec(
                "prepare",
                push,
                &prepare.argv,
                &prepare.posix,
                &prepare.windows,
            );
            commands.push((
                "prepare",
                spec,
                prepare.timeout_s.unwrap_or(DEFAULT_PREPARE_TIMEOUT_S),
            ));
            if let Some(timeout) = prepare.timeout_s
                && timeout == 0
            {
                push("prepare.timeout_s", "must be greater than 0".into());
            }
        }
        if let Some(verify) = &self.verify {
            let spec = command_spec("verify", push, &verify.argv, &verify.posix, &verify.windows);
            commands.push((
                "verify",
                spec,
                verify.timeout_s.unwrap_or(DEFAULT_VERIFY_TIMEOUT_S),
            ));
            if verify.timeout_s == Some(0) {
                push("verify.timeout_s", "must be greater than 0".into());
            }
            if verify.tail_lines == Some(0) {
                push("verify.tail_lines", "must be greater than 0".into());
            }
            for pattern in &verify.error_patterns {
                if regex::Regex::new(pattern).is_err() {
                    push(
                        "verify.error_patterns",
                        format!("{pattern:?} is not a valid regex"),
                    );
                }
            }
        }

        for (_, spec, _) in &commands {
            if let Some(spec) = spec {
                for windows in [false, true] {
                    if let Ok(argv) = spec.argv_for(windows) {
                        uses_tools |= argv.iter().any(|a| a.contains("{tools}"));
                    }
                }
            }
        }
        if uses_tools && self.tools.is_none() {
            push(
                "tools",
                "a command uses {tools} but no tools directory is set".into(),
            );
        }

        for (step, spec, _) in &commands {
            if let Some(spec) = spec {
                let vars = [
                    ("repo", "<repo>"),
                    ("tools", tools_dir.as_deref().unwrap_or("<tools>")),
                ];
                for windows in [false, true] {
                    // Missing variants are reported by `command_spec`; only placeholder errors remain.
                    if let Err(err @ SpecError::UnknownPlaceholder { .. }) =
                        spec.render(windows, &vars)
                    {
                        push(step, err.to_string());
                    }
                }
            }
        }
    }

    fn validate_hosts_and_agents(&self, push: &mut impl FnMut(&str, String)) {
        if let Err(message) = self.host_table() {
            push("hosts", message);
        }
        if let Some(prefix) = &self.branch_prefix {
            let trimmed = prefix.trim_end_matches('/');
            if !trimmed.is_empty() && !is_valid_branch(&format!("{trimmed}/x")) {
                push(
                    "branch_prefix",
                    format!("{prefix:?} is not valid in a git branch name"),
                );
            }
        }
        for (name, settings) in &self.agents {
            let field = format!("agents.{name}");
            if !KNOWN_AGENTS.contains(&name.as_str()) {
                push(
                    &field,
                    format!(
                        "unknown agent {name:?}; expected one of {}",
                        KNOWN_AGENTS.join(", ")
                    ),
                );
            }
            if let Some(model) = &settings.model
                && let Err(message) = crate::agents::check_model(model)
            {
                push(&format!("{field}.model"), message);
            }
        }
    }

    fn validate_settings(&self, push: &mut impl FnMut(&str, String)) {
        for pattern in &self.guards.deny_added_patterns {
            if regex::Regex::new(pattern).is_err() {
                push(
                    "guards.deny_added_patterns",
                    format!("{pattern:?} is not a valid regex"),
                );
            }
        }
        if self.guards.max_files_changed == 0 {
            push("guards.max_files_changed", "must be greater than 0".into());
        }
        if self.guards.max_lines_changed == 0 {
            push("guards.max_lines_changed", "must be greater than 0".into());
        }
        if !KNOWN_CI_PROVIDERS.contains(&self.ci.provider.as_str()) {
            push(
                "ci.provider",
                format!(
                    "unknown provider {:?}; expected one of {}",
                    self.ci.provider,
                    KNOWN_CI_PROVIDERS.join(", ")
                ),
            );
        }
        if !KNOWN_MERGE_MODES.contains(&self.ci.merge.as_str()) {
            push(
                "ci.merge",
                format!(
                    "unknown mode {:?}; expected one of {}",
                    self.ci.merge,
                    KNOWN_MERGE_MODES.join(", ")
                ),
            );
        }
        if self.ci.timeout_min == 0 {
            push("ci.timeout_min", "must be greater than 0".into());
        }
        if self.ci.merge == "when_green" && !self.delivery.open_mr {
            push(
                "ci.merge",
                "when_green needs delivery.open_mr = true".into(),
            );
        }
        if self.delivery.delete_branch && self.ci.merge != "when_green" {
            push(
                "delivery.delete_branch",
                "a branch is deleted only after its change is merged: needs ci.merge = \"when_green\""
                    .into(),
            );
        }
        if self.limits.agent_timeout_min == 0 {
            push("limits.agent_timeout_min", "must be greater than 0".into());
        }
    }

    /// Every command of the recipe rendered for both OSes, with sample values
    /// (`<repo>` for the repo, the absolute tools directory for `{tools}`).
    pub fn rendered_commands(&self, dir: &Path) -> Result<Vec<RenderedCommand>, SpecError> {
        let tools_dir = self
            .tools
            .as_ref()
            .map(|t| dir.join(t).display().to_string());
        let vars = [
            ("repo", "<repo>"),
            ("tools", tools_dir.as_deref().unwrap_or("<tools>")),
        ];
        let mut out = Vec::new();
        let steps = [
            ("prepare", self.prepare_spec()),
            ("verify", self.verify.as_ref().and_then(command_of_verify)),
        ];
        for (step, spec) in steps {
            if let Some(spec) = spec {
                for (os, windows) in [("posix", false), ("windows", true)] {
                    if spec.argv_for(windows).is_ok() {
                        out.push(RenderedCommand {
                            step,
                            os,
                            argv: spec.render(windows, &vars)?,
                        });
                    }
                }
            }
        }
        Ok(out)
    }
}

fn command_of_table(table: &CommandTable) -> Option<CommandSpec> {
    to_spec(&table.argv, &table.posix, &table.windows)
}

fn command_of_verify(table: &VerifyTable) -> Option<CommandSpec> {
    to_spec(&table.argv, &table.posix, &table.windows)
}

fn to_spec(
    argv: &Option<Vec<String>>,
    posix: &Option<Vec<String>>,
    windows: &Option<Vec<String>>,
) -> Option<CommandSpec> {
    match (argv, posix, windows) {
        (Some(argv), None, None) => Some(CommandSpec::Argv(argv.clone())),
        (None, None, None) => None,
        _ => Some(CommandSpec::PerOs {
            posix: posix.clone(),
            windows: windows.clone(),
        }),
    }
}

/// Builds the command for a step, reporting conflicts and empty tables.
fn command_spec(
    step: &str,
    push: &mut impl FnMut(&str, String),
    argv: &Option<Vec<String>>,
    posix: &Option<Vec<String>>,
    windows: &Option<Vec<String>>,
) -> Option<CommandSpec> {
    if argv.is_some() && (posix.is_some() || windows.is_some()) {
        push(step, "set either argv or posix/windows, not both".into());
        return None;
    }
    let spec = to_spec(argv, posix, windows);
    if spec.is_none() {
        push(step, "set argv, or posix and/or windows".into());
    }
    for list in [argv, posix, windows].into_iter().flatten() {
        if list.is_empty() {
            push(step, "argument list must not be empty".into());
        }
    }
    // A step for one OS only is refused: a run on the other OS must fail visibly, not be skipped.
    if let Some(CommandSpec::PerOs { posix, windows }) = &spec {
        if posix.is_none() {
            push(
                step,
                "has no posix variant; add posix = [...] or argv = [...]".into(),
            );
        }
        if windows.is_none() {
            push(
                step,
                "has no windows variant; add windows = [...] or argv = [...]".into(),
            );
        }
    }
    spec
}

fn check_file(dir: &Path, file: &str, field: &str, push: &mut impl FnMut(&str, String)) {
    if !is_safe_relative(file) {
        push(
            field,
            format!("{file:?} must be a relative path inside the recipe"),
        );
    } else if !dir.join(file).is_file() {
        push(
            field,
            format!("file {file} does not exist in the recipe directory"),
        );
    }
}

/// True for a relative path with no `..`, root or prefix component.
pub fn is_safe_relative(path: &str) -> bool {
    let p = Path::new(path);
    !path.is_empty()
        && !p.is_absolute()
        && !path.starts_with('/')
        && !path.starts_with('\\')
        && p.components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// The provider of a `provider/model` name: the part before the first slash.
pub fn provider_of(model: &str) -> Option<&str> {
    model
        .split_once('/')
        .map(|(provider, _)| provider)
        .filter(|provider| !provider.is_empty())
}

/// The provider an executor's model comes from. opencode names it in the model; Claude Code
/// and Copilot have one provider each, fixed by the agent (written down here, not inferred).
pub fn executor_provider(agent: &str, model: &str) -> Option<String> {
    if let Some(provider) = provider_of(model) {
        return Some(provider.to_string());
    }
    match agent {
        "claude-code" => Some("anthropic".to_string()),
        "copilot" => Some("github".to_string()),
        _ => None,
    }
}

fn is_recipe_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit())
        && name.len() <= 63
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The rules git applies to ref names, for the parts that matter here.
pub(crate) fn is_valid_branch(branch: &str) -> bool {
    !branch.is_empty()
        && !branch.starts_with('-')
        && !branch.starts_with('/')
        && !branch.ends_with('/')
        && !branch.ends_with('.')
        && !branch.ends_with(".lock")
        && !branch.contains("..")
        && !branch.contains("//")
        && !branch.contains("@{")
        && branch != "@"
        && branch.chars().all(|c| {
            !c.is_control()
                && !c.is_whitespace()
                && !matches!(c, '~' | '^' | ':' | '?' | '*' | '[' | '\\')
        })
}

fn skip_ci_marker(text: &str) -> Option<&'static str> {
    let lower = text.to_ascii_lowercase();
    SKIP_CI_MARKERS
        .iter()
        .copied()
        .find(|marker| lower.contains(marker))
}

/// JSON Schema of `recipe.toml` for this binary.
pub fn json_schema() -> String {
    let schema = schemars::schema_for!(Recipe);
    serde_json::to_string_pretty(&schema).expect("schema serialises")
}

impl Recipe {
    /// The branch fleetify pushes: the prefix, a slash, then `branch`. An empty prefix
    /// pushes `branch` as it is. A trailing slash on the prefix is ignored.
    pub fn remote_branch(&self) -> String {
        let prefix = self
            .branch_prefix
            .as_deref()
            .unwrap_or(DEFAULT_BRANCH_PREFIX)
            .trim_end_matches('/');
        if prefix.is_empty() {
            self.branch.clone()
        } else {
            format!("{prefix}/{}", self.branch)
        }
    }

    /// The hosts this recipe knows: the built-in ones plus `[hosts]`.
    pub fn host_table(&self) -> Result<HostTable, String> {
        let mut table = HostTable::with_builtins();
        for (name, entry) in &self.hosts {
            table.declare(name, entry.kind)?;
        }
        Ok(table)
    }

    /// The model the recipe itself gives `name`: its own `model` when `name` is its agent,
    /// else `[agents.<name>] model`. `--model` is applied by the caller.
    pub fn model_for(&self, name: &str) -> Option<&str> {
        let own = (self.agent.as_deref() == Some(name))
            .then_some(self.model.as_deref())
            .flatten();
        own.or_else(|| self.agents.get(name).and_then(|a| a.model.as_deref()))
    }

    /// The `prepare` step as a command, if the recipe has one.
    pub fn prepare_spec(&self) -> Option<CommandSpec> {
        self.prepare_table().as_ref().and_then(command_of_table)
    }

    /// The `prepare` step as a table: the recipe's `[prepare]`, or the `[script]` turned into
    /// the same per-OS commands.
    pub fn prepare_table(&self) -> Option<CommandTable> {
        if let Some(prepare) = &self.prepare {
            return Some(prepare.clone());
        }
        let script = self.script.as_ref()?;
        let tool = |file: &String| format!("{{tools}}/{file}");
        Some(CommandTable {
            argv: None,
            posix: script
                .posix
                .as_ref()
                .map(|file| vec!["bash".to_string(), tool(file), "{repo}".to_string()]),
            windows: script.windows.as_ref().map(|file| {
                vec![
                    "pwsh".to_string(),
                    "-NoProfile".to_string(),
                    "-ExecutionPolicy".to_string(),
                    "Bypass".to_string(),
                    "-File".to_string(),
                    tool(file),
                    "{repo}".to_string(),
                ]
            }),
            timeout_s: script.timeout_s,
        })
    }

    /// The verifier as a command, if the recipe has one.
    pub fn verify_spec(&self) -> Option<CommandSpec> {
        self.verify.as_ref().and_then(command_of_verify)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A throwaway recipe directory with the given files.
    fn fixture(name: &str, files: &[&str]) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("fleetify-recipe-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        for file in files {
            let path = dir.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            if !file.ends_with('/') {
                fs::write(&path, "x").unwrap();
            }
        }
        dir
    }

    const VALID: &str = r#"
        schema = 1
        name = "enable-gitlab-ci"
        branch = "chore/enable-gitlab-ci"
        pr_title = "chore: enable GitLab CI pipeline"
        agent = "claude-code"
        attachments = ["reference/gitlab-ci.yaml"]
        tools = "tools"

        [applies_to]
        host_type = ["gitlab"]
        not_has_file = [".gitlab-ci.yml"]

        [[instructions]]
        file = "prompts/enable-ci.md"

        [[instructions]]
        text = "Adapt the reference pipeline."

        [prepare]
        posix = ["bash", "{tools}/prepare.sh", "{repo}"]
        windows = ["pwsh", "-NoProfile", "-File", "{tools}/prepare.ps1", "{repo}"]
        timeout_s = 120

        [verify]
        posix = ["bash", "{tools}/verify.sh", "{repo}"]
        windows = ["pwsh", "-NoProfile", "-File", "{tools}/verify.ps1", "{repo}"]
        timeout_s = 300
        tail_lines = 40
        error_patterns = ['^E\s', 'error:']

        [guards]
        deny_paths = ["sonar-project.properties"]
        deny_added_patterns = ["NOSONAR", '#\s*nosec', '#\s*noqa']
        max_files_changed = 50
        max_lines_changed = 2000

        [ci]
        provider = "auto"
        wait = false
        timeout_min = 30
        merge = "off"

        [limits]
        agent_timeout_min = 60
        idle_warn_min = 10
    "#;

    fn valid_dir(name: &str) -> std::path::PathBuf {
        fixture(
            name,
            &[
                "recipe.toml",
                "prompts/enable-ci.md",
                "reference/gitlab-ci.yaml",
                "tools/verify.sh",
                "tools/verify.ps1",
                "tools/prepare.sh",
                "tools/prepare.ps1",
            ],
        )
    }

    fn issues_for(text: &str, name: &str) -> Vec<Issue> {
        let dir = valid_dir(name);
        let recipe = Recipe::parse(text).unwrap();
        recipe.validate(&dir, None)
    }

    #[test]
    fn ci_wait_is_accepted_now_that_the_gate_exists() {
        let dir = valid_dir("ci-wait");
        let text = VALID.replace("wait = false", "wait = true");
        let recipe = Recipe::parse(&text).unwrap();
        let fields: Vec<String> = recipe
            .validate(&dir, None)
            .into_iter()
            .map(|i| i.field)
            .collect();
        assert!(!fields.contains(&"ci.wait".to_string()), "{fields:?}");
    }

    #[test]
    fn delete_branch_needs_a_merge_and_merge_needs_a_change_request() {
        let dir = valid_dir("delivery");
        let fields = |text: String| -> Vec<String> {
            Recipe::parse(&text)
                .unwrap()
                .validate(&dir, None)
                .into_iter()
                .map(|i| i.field)
                .collect()
        };
        let delete = fields(format!("{VALID}\n[delivery]\ndelete_branch = true\n"));
        assert!(
            delete.contains(&"delivery.delete_branch".to_string()),
            "{delete:?}"
        );

        let no_mr = fields(format!(
            "{}\n[delivery]\nopen_mr = false\n",
            VALID.replace("merge = \"off\"", "merge = \"when_green\"")
        ));
        assert!(no_mr.contains(&"ci.merge".to_string()), "{no_mr:?}");
    }

    #[test]
    fn valid_recipe_has_no_issues_and_renders_both_oses() {
        let dir = valid_dir("valid");
        let recipe = Recipe::parse(VALID).unwrap();
        assert_eq!(recipe.validate(&dir, None), vec![]);
        let rendered = recipe.rendered_commands(&dir).unwrap();
        assert_eq!(rendered.len(), 4);
        let verify_posix = rendered
            .iter()
            .find(|c| c.step == "verify" && c.os == "posix")
            .unwrap();
        assert_eq!(verify_posix.argv[0], "bash");
        assert_eq!(verify_posix.argv[2], "<repo>");
        assert!(verify_posix.argv[1].ends_with("verify.sh"));
    }

    #[test]
    fn unknown_key_is_a_parse_error_naming_it() {
        let err = Recipe::parse(&VALID.replace("pr_title", "pr_titel")).unwrap_err();
        assert!(err.to_string().contains("pr_titel"), "{err}");
    }

    #[test]
    fn wrong_schema_version_is_reported() {
        let issues = issues_for(&VALID.replace("schema = 1", "schema = 2"), "schema2");
        assert!(issues.iter().any(|i| i.field == "schema"), "{issues:?}");
    }

    #[test]
    fn instructions_without_agent_are_refused_unless_overridden() {
        let text = VALID.replace("agent = \"claude-code\"\n", "");
        let dir = valid_dir("no-agent");
        let recipe = Recipe::parse(&text).unwrap();
        assert!(
            recipe
                .validate(&dir, None)
                .iter()
                .any(|i| i.field == "agent")
        );
        assert!(recipe.validate(&dir, Some("opencode")).is_empty());
    }

    #[test]
    fn unknown_agent_is_reported() {
        let issues = issues_for(&VALID.replace("claude-code", "gpt-pilot"), "agent");
        assert!(
            issues
                .iter()
                .any(|i| i.field == "agent" && i.message.contains("gpt-pilot"))
        );
    }

    #[test]
    fn image_must_be_an_image_name_and_needs_instructions() {
        let option_like = VALID.replace(
            "agent = \"claude-code\"\n",
            "agent = \"claude-code\"\nimage = \"--privileged\"\n",
        );
        assert!(
            issues_for(&option_like, "image")
                .iter()
                .any(|i| i.field == "image")
        );
        let image = format!(
            "{}\n[network]\nallow = [\"api.example.com:443\"]\n",
            VALID.replace(
                "agent = \"claude-code\"\n",
                "agent = \"claude-code\"\nimage = \"fleetify-agent-claude-code:1\"\n",
            )
        );
        assert!(issues_for(&image, "image").is_empty());
        let mut no_instructions = Recipe::parse(&image).unwrap();
        no_instructions.instructions.clear();
        let dir = valid_dir("image-no-instructions");
        assert!(
            no_instructions
                .validate(&dir, None)
                .iter()
                .any(|i| i.field == "image")
        );
    }

    #[test]
    fn network_needs_an_image_a_list_and_valid_entries() {
        let dir = valid_dir("network");
        let mut bare = Recipe::parse(VALID).unwrap();
        bare.network.allow = vec!["api.example.com:443".to_string()];
        assert!(
            bare.validate(&dir, None)
                .iter()
                .any(|i| i.field == "network")
        );

        let mut contained = Recipe::parse(VALID).unwrap();
        contained.image = Some("fleetify-agent-claude-code".to_string());
        assert!(
            contained
                .validate(&dir, None)
                .iter()
                .any(|i| i.field == "network"),
            "an image with no allowed host is refused"
        );

        contained.network.allow = vec![
            "10.0.0.0/24:443".to_string(),
            "llm-box.lan:8080".to_string(),
        ];
        let issues = contained.validate(&dir, None);
        assert!(
            issues
                .iter()
                .any(|i| i.field == "network.allow" && i.message.contains("10.0.0.0/24")),
            "{issues:?}"
        );
        assert!(!issues.iter().any(|i| i.message.contains("llm-box.lan")));
    }

    #[test]
    fn isolation_names_a_known_backend_and_needs_an_image() {
        let dir = valid_dir("isolation");
        let mut recipe = Recipe::parse(VALID).unwrap();
        recipe.isolation = Some("docker".to_string());
        assert!(
            recipe
                .validate(&dir, None)
                .iter()
                .any(|i| i.field == "isolation")
        );
        recipe.image = Some("fleetify-agent-claude-code".to_string());
        recipe.network.allow = vec!["api.example.com:443".to_string()];
        assert!(recipe.validate(&dir, None).is_empty());
        recipe.isolation = Some("firecracker".to_string());
        assert!(
            recipe
                .validate(&dir, None)
                .iter()
                .any(|i| i.field == "isolation")
        );
    }

    #[test]
    fn ignored_hosts_need_a_container_and_are_not_also_allowed() {
        let mut bare = Recipe::parse(VALID).unwrap();
        bare.network.ignore = vec!["telemetry.example.com:443".to_string()];
        let dir = valid_dir("ignore-bare");
        assert!(
            bare.validate(&dir, None)
                .iter()
                .any(|i| i.field == "network" && i.message.contains("ignore needs image"))
        );
        let mut contained = Recipe::parse(VALID).unwrap();
        contained.image = Some("fleetify-agent-opencode".to_string());
        contained.network.allow = vec!["api.example.com:443".to_string()];
        contained.network.ignore = vec!["api.example.com:443".to_string()];
        let dir = valid_dir("ignore-both");
        assert!(
            contained
                .validate(&dir, None)
                .iter()
                .any(|i| i.field == "network.ignore" && i.message.contains("both allow and ignore"))
        );
        contained.network.ignore = vec!["telemetry.example.com:443".to_string()];
        assert!(
            !contained
                .validate(&dir, None)
                .iter()
                .any(|i| i.field.starts_with("network"))
        );
    }

    #[test]
    fn a_script_becomes_the_prepare_step_with_the_interpreter_for_each_os() {
        let mut recipe = Recipe::parse(VALID).unwrap();
        recipe.prepare = None;
        recipe.script = Some(ScriptTable {
            posix: Some("update.sh".to_string()),
            windows: Some("update.ps1".to_string()),
            timeout_s: Some(90),
        });
        let table = recipe.prepare_table().unwrap();
        assert_eq!(
            table.posix.unwrap(),
            ["bash", "{tools}/update.sh", "{repo}"]
        );
        assert_eq!(
            table.windows.unwrap(),
            [
                "pwsh",
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                "{tools}/update.ps1",
                "{repo}"
            ]
        );
        assert_eq!(table.timeout_s, Some(90));
        assert!(table.argv.is_none());
    }

    #[test]
    fn a_script_needs_a_file_inside_the_tools_folder_and_not_both_prepare_and_script() {
        let dir = valid_dir("script-rules");
        let mut recipe = Recipe::parse(VALID).unwrap();
        recipe.prepare = None;
        recipe.script = Some(ScriptTable::default());
        assert!(
            recipe
                .validate(&dir, None)
                .iter()
                .any(|i| i.field == "script" && i.message.contains("names no file"))
        );
        recipe.script = Some(ScriptTable {
            posix: Some("missing.sh".to_string()),
            ..ScriptTable::default()
        });
        assert!(
            recipe
                .validate(&dir, None)
                .iter()
                .any(|i| i.field == "script.posix" && i.message.contains("does not exist"))
        );
        recipe.script = Some(ScriptTable {
            posix: Some("prepare.sh".to_string()),
            windows: Some("prepare.ps1".to_string()),
            timeout_s: None,
        });
        assert!(
            recipe
                .validate(&dir, None)
                .iter()
                .all(|i| !i.field.starts_with("script")),
            "two files that exist are valid"
        );
        recipe.prepare = Some(CommandTable {
            argv: Some(vec!["sh".to_string(), "{tools}/prepare.sh".to_string()]),
            ..CommandTable::default()
        });
        assert!(
            recipe
                .validate(&dir, None)
                .iter()
                .any(|i| i.field == "script" && i.message.contains("not both"))
        );
    }

    #[test]
    fn the_copilot_token_is_listed_for_a_copilot_container_and_no_other() {
        let mut copilot = Recipe::parse(VALID).unwrap();
        copilot.agent = Some("copilot".to_string());
        copilot.image = Some("fleetify-agent-copilot".to_string());
        let issues = |recipe: &Recipe| {
            let mut found = Vec::new();
            recipe.validate_agent_env(&mut |field: &str, message: String| {
                found.push((field.to_string(), message))
            });
            found
        };
        assert!(
            issues(&copilot)
                .iter()
                .any(|(_, m)| m.contains("needs COPILOT_GITHUB_TOKEN")),
            "a Copilot container must list the token"
        );
        copilot.agent_env = vec!["COPILOT_GITHUB_TOKEN".to_string()];
        assert!(issues(&copilot).is_empty());
        copilot.agent = Some("claude-code".to_string());
        assert!(
            issues(&copilot)
                .iter()
                .any(|(_, m)| m.contains("for a Copilot container only")),
            "another agent must not list the token"
        );
    }

    #[test]
    fn agent_env_needs_an_image_and_real_names_and_config_needs_opencode() {
        let mut bare = Recipe::parse(VALID).unwrap();
        bare.agent_env = vec!["OPENAI_API_KEY".to_string()];
        let dir = valid_dir("agent-env-bare");
        assert!(
            bare.validate(&dir, None)
                .iter()
                .any(|i| i.field == "agent_env")
        );

        let mut contained = Recipe::parse(VALID).unwrap();
        contained.image = Some("fleetify-agent-claude-code".to_string());
        contained.network.allow = vec!["api.example.com:443".to_string()];
        contained.agent_env = vec!["OPENAI_API_KEY".to_string()];
        assert!(contained.validate(&dir, None).is_empty());

        contained.agent_env = vec!["PATH".to_string()];
        assert!(
            contained
                .validate(&dir, None)
                .iter()
                .any(|i| i.field == "agent_env")
        );

        contained.agent_env = Vec::new();
        contained.agents.insert(
            "claude-code".to_string(),
            AgentSettings {
                model: None,
                config: Some(serde_json::json!({"provider": {}})),
            },
        );
        assert!(
            contained
                .validate(&dir, None)
                .iter()
                .any(|i| i.field == "agents.claude-code.config")
        );

        contained.agents.clear();
        contained.agents.insert(
            "opencode".to_string(),
            AgentSettings {
                model: None,
                config: Some(serde_json::json!({"permission": {}})),
            },
        );
        assert!(
            contained
                .validate(&dir, None)
                .iter()
                .any(|i| i.field == "agents.opencode.config")
        );
    }

    #[test]
    fn model_is_optional_and_an_option_like_name_is_reported() {
        let model = VALID.replace(
            "agent = \"claude-code\"\n",
            "agent = \"claude-code\"\nmodel = \"--allow-all\"\n",
        );
        let issues = issues_for(&model, "model");
        assert!(issues.iter().any(|i| i.field == "model"));
        assert!(issues_for(VALID, "model_ok").is_empty());
    }

    #[test]
    fn skip_ci_marker_in_branch_or_title_is_refused() {
        let text = VALID.replace(
            "branch = \"chore/enable-gitlab-ci\"",
            "branch = \"chore/enable-gitlab-ci-skip-ci\"",
        );
        let issues = issues_for(&text, "skip-branch");
        assert!(issues.iter().any(|i| i.field == "branch"), "{issues:?}");

        let title = VALID.replace(
            "chore: enable GitLab CI pipeline",
            "chore: update [skip ci]",
        );
        let issues = issues_for(&title, "skip-title");
        assert!(issues.iter().any(|i| i.field == "pr_title"), "{issues:?}");
    }

    #[test]
    fn commit_message_defaults_to_title_for_the_marker_check() {
        let text = VALID.replace(
            "pr_title = \"chore: enable GitLab CI pipeline\"",
            "pr_title = \"chore: x\"\ncommit_message = \"[ci skip] tidy\"",
        );
        let issues = issues_for(&text, "commit-marker");
        assert!(
            issues.iter().any(|i| i.field == "commit_message"),
            "{issues:?}"
        );
    }

    #[test]
    fn invalid_branch_names_are_refused() {
        for bad in ["", "-x", "a..b", "a b", "a~b", "x.lock", "/a", "a/"] {
            let text = VALID.replace("chore/enable-gitlab-ci", bad);
            let issues = issues_for(&text, "branch-bad");
            assert!(
                issues.iter().any(|i| i.field == "branch"),
                "{bad:?} should be refused"
            );
        }
    }

    #[test]
    fn missing_instruction_file_is_named() {
        let dir = fixture("missing-file", &["recipe.toml"]);
        let recipe = Recipe::parse(VALID).unwrap();
        let issues = recipe.validate(&dir, None);
        assert!(
            issues.iter().any(|i| i.field == "instructions[0]"),
            "{issues:?}"
        );
        assert!(
            issues.iter().any(|i| i.field == "attachments"),
            "{issues:?}"
        );
        assert!(issues.iter().any(|i| i.field == "tools"), "{issues:?}");
    }

    #[test]
    fn paths_escaping_the_recipe_are_refused() {
        let text = VALID.replace(
            "attachments = [\"reference/gitlab-ci.yaml\"]",
            "attachments = [\"../secrets.txt\"]",
        );
        let issues = issues_for(&text, "escape");
        assert!(
            issues
                .iter()
                .any(|i| i.field == "attachments" && i.message.contains("relative"))
        );
    }

    #[test]
    fn instruction_needs_exactly_one_of_file_and_text() {
        let text = VALID.replace(
            "text = \"Adapt the reference pipeline.\"",
            "file = \"prompts/enable-ci.md\"\n        text = \"x\"",
        );
        let issues = issues_for(&text, "both");
        assert!(
            issues.iter().any(|i| i.field == "instructions[1]"),
            "{issues:?}"
        );
    }

    #[test]
    fn tools_placeholder_without_tools_dir_is_refused() {
        let text = VALID.replace("tools = \"tools\"\n", "");
        let issues = issues_for(&text, "no-tools");
        assert!(
            issues
                .iter()
                .any(|i| i.field == "tools" && i.message.contains("{tools}")),
            "{issues:?}"
        );
    }

    #[test]
    fn command_with_one_os_only_is_refused_for_the_other() {
        let text = VALID.replace(
            "windows = [\"pwsh\", \"-NoProfile\", \"-File\", \"{tools}/verify.ps1\", \"{repo}\"]\n",
            "",
        );
        let issues = issues_for(&text, "posix-only");
        assert!(
            issues
                .iter()
                .any(|i| i.field == "verify" && i.message.contains("windows")),
            "{issues:?}"
        );
    }

    #[test]
    fn unknown_placeholder_in_command_names_it() {
        let text = VALID.replace(
            "{repo}\"]\n        windows",
            "{reponame}\"]\n        windows",
        );
        let issues = issues_for(&text, "placeholder");
        assert!(
            issues
                .iter()
                .any(|i| i.field == "verify" && i.message.contains("reponame")),
            "{issues:?}"
        );
    }

    #[test]
    fn invalid_regexes_are_reported_by_field() {
        let text = VALID.replace("'#\\s*noqa'", "'(unclosed'");
        let issues = issues_for(&text, "regex");
        assert!(
            issues
                .iter()
                .any(|i| i.field == "guards.deny_added_patterns"),
            "{issues:?}"
        );
    }

    #[test]
    fn zero_limits_and_bad_ci_modes_are_reported() {
        let text = VALID
            .replace("merge = \"off\"", "merge = \"always\"")
            .replace("timeout_s = 300", "timeout_s = 0");
        let issues = issues_for(&text, "numbers");
        let fields: Vec<&str> = issues.iter().map(|i| i.field.as_str()).collect();
        assert!(fields.contains(&"ci.merge"), "{fields:?}");
        assert!(fields.contains(&"verify.timeout_s"), "{fields:?}");
    }

    #[test]
    fn applies_to_conditions_report_what_is_unmet() {
        let dir = fixture(
            "applies",
            &["pyproject.toml", ".gitlab-ci.yml", "package.json"],
        );
        let recipe = Recipe::parse(
            r#"
            schema = 1
            name = "x"
            branch = "b"
            pr_title = "t"
            [applies_to]
            host_type = ["gitlab"]
            path_glob = ["team-a/*"]
            has_file = ["pyproject.toml"]
            has_file_any = ["uv.lock", "requirements.txt"]
            not_has_file = [".gitlab-ci.yml"]
            "#,
        )
        .unwrap();
        let facts = RepoFacts {
            host_type: "github",
            path: "team-b/x",
            checkout: &dir,
        };
        let unmet = recipe.unmet_conditions(&facts);
        assert_eq!(unmet.len(), 4, "{unmet:?}");

        let facts = RepoFacts {
            host_type: "gitlab",
            path: "team-a/sub/x",
            checkout: &dir,
        };
        let unmet = recipe.unmet_conditions(&facts);
        assert_eq!(unmet.len(), 2, "{unmet:?}");
    }

    #[test]
    fn empty_applies_to_matches_every_repo() {
        let dir = fixture("empty-applies", &[]);
        let recipe = Recipe::parse(VALID.split("[applies_to]").next().unwrap()).unwrap();
        let facts = RepoFacts {
            host_type: "github",
            path: "a/b",
            checkout: &dir,
        };
        assert!(recipe.unmet_conditions(&facts).is_empty());
    }

    #[test]
    fn json_schema_is_exported_and_mentions_the_schema_field() {
        let schema = json_schema();
        assert!(schema.contains("\"pr_title\""));
        assert!(schema.contains("\"instructions\""));
    }

    const MINIMAL: &str = r#"
        schema = 1
        name = "minimal"
        branch = "chore/minimal"
        pr_title = "chore: minimal"
    "#;

    #[test]
    fn a_recipe_without_settings_gets_the_defaults_and_needs_no_file() {
        let recipe = Recipe::parse(MINIMAL).unwrap();
        assert!(recipe.policy.require_checked_agent);
        assert!(recipe.needs.commands.is_empty());
        assert!(!recipe.needs.web && !recipe.needs.unrestricted);
        assert_eq!(recipe.remote_branch(), "fleetify/chore/minimal");
        assert_eq!(
            recipe.ci.dispatch_after(),
            std::time::Duration::from_secs(180)
        );
        let budget = recipe.ci.log_budget();
        assert_eq!((budget.lines, budget.chars), (100, 1000));
        assert_eq!(
            recipe.rate_limit.max_wait(),
            crate::hosts::DEFAULT_MAX_RATE_WAIT
        );
        assert_eq!(recipe.model_for("claude-code"), None);
        let hosts = recipe.host_table().unwrap();
        assert_eq!(hosts.get("github.com"), Some(HostType::Github));
        assert_eq!(hosts.get("gitlab.com"), Some(HostType::Gitlab));
        assert_eq!(hosts.get("gitlab.example.com"), None);
        assert_eq!(
            recipe.validate(&valid_dir("minimal-defaults"), None),
            vec![]
        );
    }

    #[test]
    fn branch_prefix_defaults_to_fleetify_and_can_be_empty_or_custom() {
        let custom = Recipe::parse(&format!("{MINIMAL}\nbranch_prefix = \"bot/\"\n")).unwrap();
        assert_eq!(custom.remote_branch(), "bot/chore/minimal");
        let none = Recipe::parse(&format!("{MINIMAL}\nbranch_prefix = \"\"\n")).unwrap();
        assert_eq!(none.remote_branch(), "chore/minimal");
        let bad = Recipe::parse(&format!("{MINIMAL}\nbranch_prefix = \"bad..prefix\"\n")).unwrap();
        let issues = bad.validate(&valid_dir("bad-prefix"), None);
        assert!(
            issues.iter().any(|i| i.field == "branch_prefix"),
            "{issues:?}"
        );
    }

    #[test]
    fn declared_hosts_extend_the_builtins_and_cannot_change_a_builtin_type() {
        let declared = Recipe::parse(&format!(
            "{MINIMAL}\n[hosts.\"gitlab.example.com\"]\ntype = \"gitlab\"\n"
        ))
        .unwrap();
        let table = declared.host_table().unwrap();
        assert_eq!(table.get("GitLab.example.com"), Some(HostType::Gitlab));
        assert_eq!(table.get("github.com"), Some(HostType::Github));

        let changed = Recipe::parse(&format!(
            "{MINIMAL}\n[hosts.\"github.com\"]\ntype = \"gitlab\"\n"
        ))
        .unwrap();
        assert!(changed.host_table().is_err());
        let issues = changed.validate(&valid_dir("host-changed"), None);
        assert!(issues.iter().any(|i| i.field == "hosts"), "{issues:?}");

        let with_port = Recipe::parse(&format!(
            "{MINIMAL}\n[hosts.\"https://gitlab.example.com:8443\"]\ntype = \"gitlab\"\n"
        ))
        .unwrap();
        assert!(with_port.host_table().is_err());
    }

    #[test]
    fn ci_and_rate_limit_settings_are_read_from_the_recipe() {
        let recipe = Recipe::parse(&format!(
            "{MINIMAL}\n[ci]\ndispatch_after_s = 45\nlog_lines = 20\nlog_chars = 500\n\n[rate_limit]\nmax_wait_min = 0\n"
        ))
        .unwrap();
        assert_eq!(
            recipe.ci.dispatch_after(),
            std::time::Duration::from_secs(45)
        );
        assert_eq!(recipe.ci.log_budget().lines, 20);
        assert_eq!(recipe.ci.log_budget().chars, 500);
        assert_eq!(recipe.rate_limit.max_wait(), std::time::Duration::ZERO);
        assert!(Recipe::parse(&format!("{MINIMAL}\n[ci]\ndispatch_after_min = 3\n")).is_err());
    }

    #[test]
    fn policy_and_agent_settings_are_read_and_checked() {
        let recipe = Recipe::parse(&format!(
            "{MINIMAL}\nagent = \"claude-code\"\n[agents.claude-code]\nmodel = \"from-table\"\n[agents.gpt-pilot]\nmodel = \"x\"\n[policy]\nrequire_checked_agent = false\n"
        ))
        .unwrap();
        assert!(!recipe.policy.require_checked_agent);
        assert_eq!(recipe.model_for("claude-code"), Some("from-table"));
        let issues = recipe.validate(&valid_dir("agents-table"), None);
        assert!(
            issues.iter().any(|i| i.field == "agents.gpt-pilot"),
            "{issues:?}"
        );
    }

    #[test]
    fn needs_unrestricted_is_read_and_defaults_to_off() {
        let asks = Recipe::parse(&format!("{MINIMAL}\n[needs]\nunrestricted = true\n")).unwrap();
        assert!(asks.needs.unrestricted);
        assert!(!Recipe::parse(MINIMAL).unwrap().needs.unrestricted);
    }

    #[test]
    fn branch_and_name_rules() {
        assert!(is_valid_branch("chore/enable-gitlab-ci"));
        assert!(is_recipe_name("enable-gitlab-ci"));
        assert!(!is_recipe_name("Enable"));
        assert!(!is_recipe_name("-x"));
        assert!(is_safe_relative("a/b.txt"));
        assert!(!is_safe_relative("/abs"));
        assert!(!is_safe_relative("a/../b"));
    }
}

#[cfg(test)]
mod judge_tests {
    use super::*;

    /// A scripted recipe (no instructions, so no agent unless `agent` is given) with `judge`
    /// appended as its `[judge]` table.
    fn recipe(agent: Option<&str>, model: Option<&str>, judge: &str) -> Recipe {
        let mut text = String::from(
            "schema = 1\nname = \"judge-check\"\nbranch = \"m16/judge\"\npr_title = \"judge check\"\n",
        );
        if let Some(agent) = agent {
            text.push_str(&format!("agent = \"{agent}\"\n"));
        }
        if let Some(model) = model {
            text.push_str(&format!("model = \"{model}\"\n"));
        }
        text.push_str(judge);
        Recipe::parse(&text).unwrap()
    }

    fn fields(recipe: &Recipe) -> Vec<String> {
        let dir = std::env::temp_dir();
        recipe
            .validate(&dir, None)
            .into_iter()
            .map(|issue| format!("{}: {}", issue.field, issue.message))
            .filter(|line| line.starts_with("judge") || line.starts_with("model"))
            .collect()
    }

    const TASK: &str = "\n[judge]\ntask = \"make the test pass\"\n";

    #[test]
    fn no_judge_section_means_no_judge() {
        let r = recipe(None, None, "");
        assert!(r.judge.is_none());
        assert!(fields(&r).is_empty());
    }

    #[test]
    fn level_and_max_diff_lines_are_required_with_the_section() {
        let r = recipe(None, None, "\n[judge]\ntask = \"x\"\n");
        let out = fields(&r);
        assert!(out.iter().any(|l| l.starts_with("judge.level")), "{out:?}");
        assert!(
            out.iter().any(|l| l.starts_with("judge.max_diff_lines")),
            "{out:?}"
        );
    }

    #[test]
    fn an_unknown_level_is_refused() {
        let r = recipe(
            None,
            None,
            "\n[judge]\nlevel = \"team\"\nmax_diff_lines = 1000\ncontext_tokens = 32768\noutput_tokens = 8192\ntask = \"x\"\n",
        );
        assert!(fields(&r).iter().any(|l| l.starts_with("judge.level")));
    }

    #[test]
    fn a_zero_size_limit_is_refused() {
        let r = recipe(
            None,
            None,
            "\n[judge]\nlevel = \"provider\"\nmax_diff_lines = 0\ncontext_tokens = 32768\noutput_tokens = 8192\nagent = \"opencode\"\nmodel = \"llamacpp/qwen\"\ntask = \"x\"\n",
        );
        assert!(
            fields(&r)
                .iter()
                .any(|l| l.starts_with("judge.max_diff_lines"))
        );
    }

    #[test]
    fn a_scripted_recipe_may_use_provider_with_another_provider_and_a_task() {
        let r = recipe(
            None,
            None,
            "\n[judge]\nlevel = \"provider\"\nmax_diff_lines = 1000\ncontext_tokens = 32768\noutput_tokens = 8192\nagent = \"opencode\"\nmodel = \"llamacpp/qwen3.8-flash-next\"\ntask = \"x\"\n",
        );
        assert!(fields(&r).is_empty(), "{:?}", fields(&r));
    }

    #[test]
    fn session_rejects_its_own_agent_and_model_and_needs_an_executor() {
        let with_fields = recipe(
            None,
            None,
            "\n[judge]\nlevel = \"session\"\nmax_diff_lines = 1000\ncontext_tokens = 32768\noutput_tokens = 8192\nagent = \"opencode\"\nmodel = \"llamacpp/x\"\ntask = \"x\"\n",
        );
        let out = fields(&with_fields);
        assert!(out.iter().any(|l| l.starts_with("judge.agent")), "{out:?}");
        assert!(out.iter().any(|l| l.starts_with("judge.model")), "{out:?}");
        assert!(out.iter().any(|l| l.starts_with("judge.level")), "{out:?}");

        let bare = recipe(
            None,
            None,
            "\n[judge]\nlevel = \"session\"\nmax_diff_lines = 1000\ncontext_tokens = 32768\noutput_tokens = 8192\ntask = \"x\"\n",
        );
        assert!(fields(&bare).iter().any(|l| l.starts_with("judge.level")));

        let with_executor = recipe(
            Some("claude-code"),
            None,
            "\n[judge]\nlevel = \"session\"\nmax_diff_lines = 1000\ncontext_tokens = 32768\noutput_tokens = 8192\ntask = \"x\"\n",
        );
        assert!(
            fields(&with_executor).is_empty(),
            "{:?}",
            fields(&with_executor)
        );
    }

    #[test]
    fn model_and_provider_need_a_judge_agent_and_a_provider_model() {
        let no_agent = recipe(
            None,
            None,
            "\n[judge]\nlevel = \"model\"\nmax_diff_lines = 1000\ncontext_tokens = 32768\noutput_tokens = 8192\nmodel = \"llamacpp/x\"\ntask = \"x\"\n",
        );
        assert!(
            fields(&no_agent)
                .iter()
                .any(|l| l.starts_with("judge.agent"))
        );

        let bare_model = recipe(
            None,
            None,
            "\n[judge]\nlevel = \"model\"\nmax_diff_lines = 1000\ncontext_tokens = 32768\noutput_tokens = 8192\nagent = \"opencode\"\nmodel = \"qwen\"\ntask = \"x\"\n",
        );
        assert!(
            fields(&bare_model)
                .iter()
                .any(|l| l.starts_with("judge.model"))
        );
    }

    #[test]
    fn only_opencode_judges_for_now() {
        let r = recipe(
            None,
            None,
            "\n[judge]\nlevel = \"model\"\nmax_diff_lines = 1000\ncontext_tokens = 32768\noutput_tokens = 8192\nagent = \"claude-code\"\nmodel = \"sonnet\"\ntask = \"x\"\n",
        );
        assert!(
            fields(&r)
                .iter()
                .any(|l| l.starts_with("judge.agent") && l.contains("opencode only"))
        );
    }

    #[test]
    fn model_level_must_differ_from_the_executors_model() {
        let same = recipe(
            Some("opencode"),
            Some("llamacpp/qwen3.8-flash-next"),
            "\n[judge]\nlevel = \"model\"\nmax_diff_lines = 1000\ncontext_tokens = 32768\noutput_tokens = 8192\nagent = \"opencode\"\nmodel = \"llamacpp/qwen3.8-flash-next\"\ntask = \"x\"\n",
        );
        assert!(
            fields(&same)
                .iter()
                .any(|l| l.starts_with("judge.model") && l.contains("differ"))
        );
    }

    #[test]
    fn provider_level_must_come_from_another_provider() {
        let same = recipe(
            Some("opencode"),
            Some("llamacpp/qwen3.8-flash-next"),
            "\n[judge]\nlevel = \"provider\"\nmax_diff_lines = 1000\ncontext_tokens = 32768\noutput_tokens = 8192\nagent = \"opencode\"\nmodel = \"llamacpp/qwen3-coder-30b\"\ntask = \"x\"\n",
        );
        assert!(
            fields(&same)
                .iter()
                .any(|l| l.starts_with("judge.model") && l.contains("another one"))
        );

        let other = recipe(
            Some("opencode"),
            Some("llamacpp/qwen3.8-flash-next"),
            "\n[judge]\nlevel = \"provider\"\nmax_diff_lines = 1000\ncontext_tokens = 32768\noutput_tokens = 8192\nagent = \"opencode\"\nmodel = \"openai/gpt-5-nano\"\ntask = \"x\"\n",
        );
        assert!(fields(&other).is_empty(), "{:?}", fields(&other));
    }

    #[test]
    fn the_executors_agent_default_cannot_be_compared() {
        // The recipe names an agent but no model: the provider is not known, so the level is refused.
        let r = recipe(
            Some("opencode"),
            None,
            "\n[judge]\nlevel = \"provider\"\nmax_diff_lines = 1000\ncontext_tokens = 32768\noutput_tokens = 8192\nagent = \"opencode\"\nmodel = \"openai/gpt-5-nano\"\ntask = \"x\"\n",
        );
        assert!(
            fields(&r).iter().any(|l| l.starts_with("model:")),
            "{:?}",
            fields(&r)
        );
    }

    #[test]
    fn the_judge_needs_a_task_from_judge_or_instructions() {
        let r = recipe(
            None,
            None,
            "\n[judge]\nlevel = \"provider\"\nmax_diff_lines = 1000\ncontext_tokens = 32768\noutput_tokens = 8192\nagent = \"opencode\"\nmodel = \"llamacpp/x\"\n",
        );
        assert!(fields(&r).iter().any(|l| l.starts_with("judge.task")));
        let _ = TASK;
    }

    #[test]
    fn the_context_and_the_answer_reserve_are_required_and_the_answer_must_fit() {
        let missing = recipe(
            None,
            None,
            "\n[judge]\nlevel = \"provider\"\nmax_diff_lines = 10000\nagent = \"opencode\"\nmodel = \"llamacpp/x\"\ntask = \"x\"\n",
        );
        let out = fields(&missing);
        assert!(
            out.iter().any(|l| l.starts_with("judge.context_tokens")),
            "{out:?}"
        );
        assert!(
            out.iter().any(|l| l.starts_with("judge.output_tokens")),
            "{out:?}"
        );
        let too_big = recipe(
            None,
            None,
            "\n[judge]\nlevel = \"provider\"\nmax_diff_lines = 10000\ncontext_tokens = 8192\noutput_tokens = 8192\nagent = \"opencode\"\nmodel = \"llamacpp/x\"\ntask = \"x\"\n",
        );
        assert!(
            fields(&too_big)
                .iter()
                .any(|l| l.starts_with("judge.output_tokens"))
        );
    }

    #[test]
    fn the_judge_timeout_is_optional_and_bounded() {
        let base = "\n[judge]\nlevel = \"provider\"\nmax_diff_lines = 10000\ncontext_tokens = 32768\noutput_tokens = 8192\nagent = \"opencode\"\nmodel = \"llamacpp/x\"\ntask = \"x\"\n";
        assert!(fields(&recipe(None, None, base)).is_empty());
        assert!(fields(&recipe(None, None, &format!("{base}timeout_s = 3600\n"))).is_empty());
        assert!(fields(&recipe(None, None, &format!("{base}timeout_s = 7200\n"))).is_empty());
        assert!(
            fields(&recipe(None, None, &format!("{base}timeout_s = 0\n")))
                .iter()
                .any(|l| l.starts_with("judge.timeout_s"))
        );
        assert!(
            fields(&recipe(None, None, &format!("{base}timeout_s = 7201\n")))
                .iter()
                .any(|l| l.starts_with("judge.timeout_s"))
        );
        assert_eq!(recipe(None, None, base).judge.unwrap().timeout_s, None);
    }

    #[test]
    fn providers_are_read_from_the_model_name_or_fixed_by_the_agent() {
        assert_eq!(
            executor_provider("opencode", "llamacpp/qwen"),
            Some("llamacpp".to_string())
        );
        assert_eq!(
            executor_provider("claude-code", "sonnet"),
            Some("anthropic".to_string())
        );
        assert_eq!(
            executor_provider("copilot", "gpt-5"),
            Some("github".to_string())
        );
        assert_eq!(executor_provider("opencode", "qwen"), None);
        assert_eq!(provider_of("/x"), None);
    }
}
