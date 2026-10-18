//! `config.toml`: hosts, policy and permissions. It holds no secrets; unknown
//! keys are rejected, so a token written here fails loudly instead of being
//! read silently. Tokens live in `credentials.toml` (see [`crate::credentials`]).

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
    #[serde(default)]
    pub policy: Policy,
    #[serde(default)]
    pub permissions: Permissions,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HostEntry {
    #[serde(rename = "type")]
    pub kind: HostType,
}

#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    /// Refuse agents that did not pass `check-agent`.
    #[serde(default)]
    pub require_checked_agent: bool,
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

impl Config {
    /// Parses config text. Unknown keys are errors.
    pub fn parse(text: &str) -> Result<Config, ConfigError> {
        toml::from_str(text).map_err(|err| ConfigError::Parse {
            path: None,
            message: err.message().to_string(),
        })
    }

    /// Loads the file. A missing file means the defaults.
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        match fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|err| ConfigError::Parse {
                path: Some(path.to_path_buf()),
                message: err.message().to_string(),
            }),
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
        assert!(!config.policy.require_checked_agent);
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
}
