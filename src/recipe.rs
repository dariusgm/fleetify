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
}

/// `[network]`: the hosts an agent container may reach. Needs `image`, and is required when
/// `image` is set, since the agent must reach its model.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Network {
    /// `host:port` entries. The host is a DNS name or an IPv4 address; names are looked up
    /// again when a connection opens. No ranges, wildcards or schemes.
    #[serde(default)]
    pub allow: Vec<String>,
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

    fn validate_commands(&self, dir: &Path, push: &mut impl FnMut(&str, String)) {
        let tools_dir = self
            .tools
            .as_ref()
            .map(|t| dir.join(t).display().to_string());
        let mut uses_tools = false;

        let mut commands: Vec<(&str, Option<CommandSpec>, u64)> = Vec::new();
        if let Some(prepare) = &self.prepare {
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
            ("prepare", self.prepare.as_ref().and_then(command_of_table)),
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
        self.prepare.as_ref().and_then(command_of_table)
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
