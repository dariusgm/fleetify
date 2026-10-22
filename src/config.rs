//! `config.toml`: hosts, policy and permissions. It holds no secrets; unknown
//! keys are rejected, so a token written here fails loudly instead of being
//! read silently. Tokens live in `credentials.toml` (see [`crate::credentials`]).

use crate::credentials::glob_match;
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Kind of Git host. Decides which REST API and CI integration are used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HostType {
    Github,
    Gitlab,
}

impl HostType {
    pub fn as_str(self) -> &'static str {
        match self {
            HostType::Github => "github",
            HostType::Gitlab => "gitlab",
        }
    }
}

impl fmt::Display for HostType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Hosts beyond `github.com` and `gitlab.com`, keyed by host name.
    #[serde(default)]
    pub hosts: BTreeMap<String, HostEntry>,
    /// Prefix for every branch fleetify pushes: `<prefix>/<recipe branch>`. Unset means
    /// `fleetify`; an empty string means no prefix.
    #[serde(default)]
    pub branch_prefix: Option<String>,
    #[serde(default)]
    pub policy: Policy,
    #[serde(default)]
    pub merge_policy: Vec<MergeRule>,
    #[serde(default)]
    pub permissions: Permissions,
    #[serde(default)]
    pub rate_limit: RateLimit,
    #[serde(default)]
    pub ci: CiConfig,
    /// Defaults per agent, keyed by agent name. `--model` and the recipe take precedence.
    #[serde(default)]
    pub agents: BTreeMap<String, AgentConfig>,
}

/// `[agents.<name>]`: defaults for one agent.
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AgentConfig {
    /// Model passed to the agent CLI unchanged. Unset leaves the choice to the agent.
    #[serde(default)]
    pub model: Option<String>,
}

/// Unset `ci.dispatch_after_s`: three minutes.
const DEFAULT_DISPATCH_AFTER_S: u64 = 3 * 60;

/// `[ci]`: when fleetify starts CI itself because the host started none for the pushed commit.
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CiConfig {
    /// Seconds to wait for the host's own CI on the pushed commit. If none has started by
    /// then, fleetify starts it: `workflow_dispatch` on GitHub, a pipeline on GitLab. `0`
    /// starts it at once, without asking the host first. Unset means 180.
    #[serde(default)]
    pub dispatch_after_s: Option<u64>,
}

impl CiConfig {
    /// The wait before fleetify starts CI itself.
    pub fn dispatch_after(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.dispatch_after_s.unwrap_or(DEFAULT_DISPATCH_AFTER_S))
    }
}

/// `[rate_limit]`: how long fleetify waits when a host asks it to hold back.
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
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

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HostEntry {
    #[serde(rename = "type")]
    pub kind: HostType,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    /// Refuse agents that did not pass `check-agent`. On by default: an agent that was
    /// never checked may write outside its workspace (see DECISIONS.md, confinement).
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

fn default_true() -> bool {
    true
}

/// Merge permissions for the repositories whose `<host>/<path>` matches `pattern`.
/// Rules are checked in order and the first match decides. A repository without a
/// matching rule is never merged without a CI gate.
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MergeRule {
    /// Glob on `<host>/<path>`, e.g. `github.com/owner/*`.
    #[serde(rename = "match")]
    pub pattern: String,
    /// Merge with `--merge` without a CI gate. Only the user can grant this, here.
    #[serde(default)]
    pub merge_without_ci: bool,
    /// Delete the source branch after a merge. The recipe must ask for it too.
    #[serde(default)]
    pub delete_branch: bool,
}

impl Config {
    /// The first merge rule whose pattern matches `key` (`<host>/<path>`).
    pub fn merge_rule(&self, key: &str) -> Option<&MergeRule> {
        self.merge_policy
            .iter()
            .find(|rule| glob_match(&rule.pattern, key))
    }
}

/// Permissions granted to every agent, on top of the built-in baseline.
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Permissions {
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub web: bool,
    #[serde(default)]
    pub extra_directories: Vec<String>,
    #[serde(default)]
    pub agent: BTreeMap<String, AgentPermissions>,
}

#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AgentPermissions {
    /// Use the agent CLI's blanket "allow everything" option. At the user's own risk.
    #[serde(default)]
    pub unrestricted: bool,
}

#[derive(Debug)]
pub enum ConfigError {
    Read {
        path: PathBuf,
        source: io::Error,
    },
    Parse {
        path: Option<PathBuf>,
        message: String,
    },
    Invalid(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Read { path, source } => {
                write!(f, "cannot read {}: {source}", path.display())
            }
            ConfigError::Parse {
                path: Some(path),
                message,
            } => write!(f, "invalid config {}: {message}", path.display()),
            ConfigError::Parse {
                path: None,
                message,
            } => write!(f, "invalid config: {message}"),
            ConfigError::Invalid(message) => write!(f, "invalid config: {message}"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// The branch prefix when `config.toml` does not set one.
pub const DEFAULT_BRANCH_PREFIX: &str = "fleetify";

impl Config {
    /// Parses config text. Unknown keys are errors.
    pub fn parse(text: &str) -> Result<Config, ConfigError> {
        let config: Config = toml::from_str(text).map_err(|err| ConfigError::Parse {
            path: None,
            message: err.message().to_string(),
        })?;
        config
            .check_branch_prefix()
            .map_err(|message| ConfigError::Parse {
                path: None,
                message,
            })?;
        Ok(config)
    }

    /// The branch fleetify pushes for a recipe's branch: the prefix, a slash, then the
    /// recipe's name. A trailing slash on the prefix is ignored.
    pub fn branch_name(&self, recipe_branch: &str) -> String {
        let prefix = self.prefix().trim_end_matches('/');
        if prefix.is_empty() {
            recipe_branch.to_string()
        } else {
            format!("{prefix}/{recipe_branch}")
        }
    }

    fn prefix(&self) -> &str {
        self.branch_prefix
            .as_deref()
            .unwrap_or(DEFAULT_BRANCH_PREFIX)
    }

    /// The prefix must make a valid branch name when joined with a name.
    fn check_branch_prefix(&self) -> Result<(), String> {
        let prefix = self.prefix().trim_end_matches('/');
        if prefix.is_empty() || crate::recipe::is_valid_branch(&format!("{prefix}/x")) {
            Ok(())
        } else {
            Err(format!(
                "branch_prefix {prefix:?} is not valid in a git branch name"
            ))
        }
    }

    /// Loads the file. A missing file means the defaults.
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        match fs::read_to_string(path) {
            Ok(text) => {
                let config: Config = toml::from_str(&text).map_err(|err| ConfigError::Parse {
                    path: Some(path.to_path_buf()),
                    message: err.message().to_string(),
                })?;
                config
                    .check_branch_prefix()
                    .map_err(|message| ConfigError::Parse {
                        path: Some(path.to_path_buf()),
                        message,
                    })?;
                Ok(config)
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Config::default()),
            Err(source) => Err(ConfigError::Read {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    /// The host table: built-in hosts plus the configured ones.
    pub fn host_table(&self) -> Result<HostTable, ConfigError> {
        let mut table = HostTable::with_builtins();
        for (name, entry) in &self.hosts {
            if !is_valid_host_name(name) {
                return Err(ConfigError::Invalid(format!(
                    "host name {name:?} must be lowercase letters, digits, '.' or '-' (no scheme, path or port)"
                )));
            }
            match table.get(name) {
                Some(existing) if existing != entry.kind => {
                    return Err(ConfigError::Invalid(format!(
                        "host {name} is built in as {existing} and cannot be configured as {}",
                        entry.kind
                    )));
                }
                _ => {
                    table.hosts.insert(name.clone(), entry.kind);
                }
            }
        }
        Ok(table)
    }
}

/// Maps host names to host kinds. Only mapped hosts are accepted in `repos.txt`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostTable {
    hosts: BTreeMap<String, HostType>,
}

impl HostTable {
    pub fn with_builtins() -> Self {
        let mut hosts = BTreeMap::new();
        hosts.insert("github.com".to_string(), HostType::Github);
        hosts.insert("gitlab.com".to_string(), HostType::Gitlab);
        HostTable { hosts }
    }

    /// Lookup is case-insensitive.
    pub fn get(&self, host: &str) -> Option<HostType> {
        self.hosts.get(&host.to_ascii_lowercase()).copied()
    }
}

/// Lowercase letters, digits, `.` and `-`, with at least one letter or digit.
pub fn is_valid_host_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-')
}

/// Expands a leading `~/` (or a bare `~`) to the home directory.
pub fn expand_home(path: &str) -> PathBuf {
    let home = || {
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
    };
    if path == "~" {
        return home().unwrap_or_else(|| PathBuf::from(path));
    }
    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\"))
        && let Some(home) = home()
    {
        return home.join(rest);
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_has_restrictive_defaults() {
        let config = Config::parse("").unwrap();
        assert!(config.policy.require_checked_agent);
        assert!(!config.permissions.web);
        assert!(config.permissions.commands.is_empty());
        assert!(config.permissions.agent.is_empty());
    }

    #[test]
    fn configured_hosts_are_added_to_builtins() {
        let config = Config::parse(
            r#"
            [hosts."gitlab.example.com"]
            type = "gitlab"
            "#,
        )
        .unwrap();
        let table = config.host_table().unwrap();
        assert_eq!(table.get("GitLab.example.com"), Some(HostType::Gitlab));
        assert_eq!(table.get("github.com"), Some(HostType::Github));
        assert_eq!(table.get("unknown.example"), None);
    }

    #[test]
    fn builtin_host_cannot_change_type() {
        let config = Config::parse(
            r#"
            [hosts."github.com"]
            type = "gitlab"
            "#,
        )
        .unwrap();
        assert!(config.host_table().is_err());
    }

    #[test]
    fn host_name_with_scheme_or_port_is_rejected() {
        let config = Config::parse(
            r#"
            [hosts."https://gitlab.example.com:8443"]
            type = "gitlab"
            "#,
        )
        .unwrap();
        assert!(config.host_table().is_err());
    }

    #[test]
    fn token_in_config_is_rejected_as_unknown_key() {
        let err = Config::parse(
            r#"
            [permissions]
            token = "glpat-secret"
            "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("token"), "{err}");
    }

    #[test]
    fn branches_get_the_fleetify_prefix_unless_configured() {
        let default = Config::default();
        assert_eq!(default.branch_name("chore/x"), "fleetify/chore/x");
        let none = Config::parse("branch_prefix = \"\"").unwrap();
        assert_eq!(none.branch_name("chore/x"), "chore/x");
        let custom = Config::parse("branch_prefix = \"bot/\"").unwrap();
        assert_eq!(custom.branch_name("chore/x"), "bot/chore/x");
    }

    #[test]
    fn an_invalid_branch_prefix_is_refused_at_parse_time() {
        let err = Config::parse("branch_prefix = \"bad..prefix\"").unwrap_err();
        assert!(err.to_string().contains("branch_prefix"), "{err}");
    }

    #[test]
    fn missing_file_loads_defaults() {
        let path = std::env::temp_dir().join("fleetify-no-such-config-file.toml");
        let _ = fs::remove_file(&path);
        assert_eq!(Config::load(&path).unwrap(), Config::default());
    }

    #[test]
    fn home_expansion_keeps_other_paths() {
        assert_eq!(expand_home("/abs/path"), PathBuf::from("/abs/path"));
        assert_eq!(expand_home("relative"), PathBuf::from("relative"));
    }

    #[test]
    fn merge_rules_match_repositories_and_the_first_match_wins() {
        let config: Config = toml::from_str(
            r#"
            [[merge_policy]]
            match = "github.com/owner/docs"
            merge_without_ci = true
            delete_branch = true

            [[merge_policy]]
            match = "github.com/owner/*"
            merge_without_ci = false
            "#,
        )
        .unwrap();
        let docs = config.merge_rule("github.com/owner/docs").unwrap();
        assert!(docs.merge_without_ci && docs.delete_branch);
        let other = config.merge_rule("github.com/owner/service").unwrap();
        assert!(!other.merge_without_ci);
        assert!(config.merge_rule("gitlab.com/owner/service").is_none());
    }

    #[test]
    fn ci_dispatch_wait_defaults_to_three_minutes_and_zero_starts_at_once() {
        let unset: Config = toml::from_str("").unwrap();
        assert_eq!(
            unset.ci.dispatch_after(),
            std::time::Duration::from_secs(180)
        );
        let zero: Config = toml::from_str("[ci]\ndispatch_after_s = 0\n").unwrap();
        assert_eq!(zero.ci.dispatch_after(), std::time::Duration::ZERO);
        let short: Config = toml::from_str("[ci]\ndispatch_after_s = 45\n").unwrap();
        assert_eq!(
            short.ci.dispatch_after(),
            std::time::Duration::from_secs(45)
        );
        assert!(toml::from_str::<Config>("[ci]\ndispatch_after_min = 3\n").is_err());
    }

    #[test]
    fn rate_limit_ceiling_defaults_to_fifteen_minutes_and_zero_means_never() {
        let unset: Config = toml::from_str("").unwrap();
        assert_eq!(
            unset.rate_limit.max_wait(),
            crate::hosts::DEFAULT_MAX_RATE_WAIT
        );
        let never: Config = toml::from_str("[rate_limit]\nmax_wait_min = 0\n").unwrap();
        assert_eq!(never.rate_limit.max_wait(), std::time::Duration::ZERO);
        let short: Config = toml::from_str("[rate_limit]\nmax_wait_min = 2\n").unwrap();
        assert_eq!(
            short.rate_limit.max_wait(),
            std::time::Duration::from_secs(120)
        );
    }
}
