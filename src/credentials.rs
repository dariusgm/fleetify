//! `credentials.toml`: host tokens, kept apart from `config.toml`.
//!
//! Rules are checked in file order and the first match wins, so specific
//! patterns go first. A pattern is a glob on `<host>/<path>`, where `*` matches
//! any run of characters, including `/`. Tokens are never printed: [`Secret`]
//! has a redacting `Debug` and no `Display`.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// A token. Read it with [`Secret::expose`] only at the point of use.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Secret(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialRule {
    pattern: String,
    token: Secret,
}

impl CredentialRule {
    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    pub fn token(&self) -> &Secret {
        &self.token
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Credentials {
    rules: Vec<CredentialRule>,
}

#[derive(Debug)]
pub enum CredentialsError {
    Read {
        path: PathBuf,
        source: io::Error,
    },
    Parse {
        path: Option<PathBuf>,
        message: String,
    },
    Invalid {
        index: usize,
        message: String,
    },
    /// The file is readable by group or others (POSIX only).
    TooOpen {
        path: PathBuf,
        mode: u32,
    },
    /// No rule matches the repo. Lists the patterns that were checked, never a value.
    NoMatch {
        key: String,
        checked: Vec<String>,
    },
}

impl fmt::Display for CredentialsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CredentialsError::Read { path, source } => {
                write!(f, "cannot read {}: {source}", path.display())
            }
            CredentialsError::Parse {
                path: Some(path),
                message,
            } => write!(f, "invalid credentials file {}: {message}", path.display()),
            CredentialsError::Parse {
                path: None,
                message,
            } => {
                write!(f, "invalid credentials: {message}")
            }
            CredentialsError::Invalid { index, message } => {
                write!(f, "credential rule {} is invalid: {message}", index + 1)
            }
            CredentialsError::TooOpen { path, mode } => write!(
                f,
                "{} is readable by group or others (mode {:o}); restrict it to the owner, e.g. chmod 600",
                path.display(),
                mode & 0o777
            ),
            CredentialsError::NoMatch { key, checked } => {
                if checked.is_empty() {
                    write!(
                        f,
                        "no credential rule for {key}: credentials.toml has no rules"
                    )
                } else {
                    write!(
                        f,
                        "no credential rule matches {key}; rules checked: {}",
                        checked.join(", ")
                    )
                }
            }
        }
    }
}

impl std::error::Error for CredentialsError {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    #[serde(default)]
    credential: Vec<RawRule>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    #[serde(rename = "match")]
    pattern: String,
    token: String,
}

impl Credentials {
    /// Parses credentials text. Each pattern must start with a concrete host name
    /// (for example `github.com/*`), and tokens must not be empty.
    pub fn parse(text: &str) -> Result<Credentials, CredentialsError> {
        let raw: RawFile = toml::from_str(text).map_err(|err| CredentialsError::Parse {
            path: None,
            message: err.message().to_string(),
        })?;
        Self::from_raw(raw)
    }

    fn from_raw(raw: RawFile) -> Result<Credentials, CredentialsError> {
        let mut rules = Vec::with_capacity(raw.credential.len());
        for (index, rule) in raw.credential.into_iter().enumerate() {
            let invalid = |message: &str| CredentialsError::Invalid {
                index,
                message: message.to_string(),
            };
            let host = rule.pattern.split('/').next().unwrap_or("");
            if !is_host_like(host) {
                return Err(invalid(
                    "match must start with a host name, e.g. github.com/owner/* or gitlab.example.com/*",
                ));
            }
            if rule.token.trim().is_empty() {
                return Err(invalid("token is empty"));
            }
            rules.push(CredentialRule {
                pattern: rule.pattern,
                token: Secret::new(rule.token),
            });
        }
        Ok(Credentials { rules })
    }

    /// Loads the file and returns it with any warnings. A missing file is an
    /// empty rule set. On POSIX a file readable by group or others is an error.
    /// On Windows the ACL cannot be checked here, so a warning is returned.
    pub fn load(path: &Path) -> Result<(Credentials, Vec<String>), CredentialsError> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Ok((Credentials::default(), Vec::new()));
            }
            Err(source) => {
                return Err(CredentialsError::Read {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        let mut warnings = Vec::new();
        check_permissions(path, &mut warnings)?;
        let raw: RawFile = toml::from_str(&text).map_err(|err| CredentialsError::Parse {
            path: Some(path.to_path_buf()),
            message: err.message().to_string(),
        })?;
        Ok((Self::from_raw(raw)?, warnings))
    }

    /// The first rule whose pattern matches `<host>/<path>`.
    pub fn find_rule(&self, host: &str, path: &str) -> Result<&CredentialRule, CredentialsError> {
        let key = format!("{host}/{path}");
        self.rules
            .iter()
            .find(|rule| glob_match(&rule.pattern, &key))
            .ok_or_else(|| CredentialsError::NoMatch {
                key,
                checked: self.rules.iter().map(|r| r.pattern.clone()).collect(),
            })
    }

    /// The token of the first rule whose pattern matches `<host>/<path>`.
    pub fn find(&self, host: &str, path: &str) -> Result<&Secret, CredentialsError> {
        self.find_rule(host, path).map(|rule| &rule.token)
    }

    pub fn rules(&self) -> &[CredentialRule] {
        &self.rules
    }

    /// Every token value, for the redactor.
    pub fn secrets(&self) -> impl Iterator<Item = &str> {
        self.rules.iter().map(|rule| rule.token.expose())
    }
}

#[cfg(unix)]
fn check_permissions(path: &Path, _warnings: &mut Vec<String>) -> Result<(), CredentialsError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(path)
        .map_err(|source| CredentialsError::Read {
            path: path.to_path_buf(),
            source,
        })?
        .permissions()
        .mode();
    if mode & 0o077 != 0 {
        return Err(CredentialsError::TooOpen {
            path: path.to_path_buf(),
            mode,
        });
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_permissions(path: &Path, warnings: &mut Vec<String>) -> Result<(), CredentialsError> {
    warnings.push(format!(
        "cannot verify the access list of {}; restrict it to your user account",
        path.display()
    ));
    Ok(())
}

/// A concrete host name: lowercase, contains a dot, no wildcard.
fn is_host_like(host: &str) -> bool {
    host.contains('.') && crate::config::is_valid_host_name(host)
}

/// Glob with `*` as the only metacharacter; `*` also matches `/`.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<usize> = None;
    let mut resume = 0;
    while ti < t.len() {
        if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            pi += 1;
            resume = ti;
        } else if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if let Some(star_at) = star {
            pi = star_at + 1;
            resume += 1;
            ti = resume;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
        [[credential]]
        match = "gitlab.example.com/team-a/project-x"
        token = "token-single"

        [[credential]]
        match = "gitlab.example.com/team-a/*"
        token = "token-group"

        [[credential]]
        match = "gitlab.example.com/*"
        token = "token-fallback"
    "#;

    #[test]
    fn first_matching_rule_wins_not_the_longest_pattern() {
        let creds = Credentials::parse(SAMPLE).unwrap();
        assert_eq!(
            creds
                .find("gitlab.example.com", "team-a/project-x")
                .unwrap()
                .expose(),
            "token-single"
        );
        assert_eq!(
            creds
                .find("gitlab.example.com", "team-a/sub/other")
                .unwrap()
                .expose(),
            "token-group"
        );
        assert_eq!(
            creds
                .find("gitlab.example.com", "team-b/x")
                .unwrap()
                .expose(),
            "token-fallback"
        );
    }

    #[test]
    fn no_match_names_the_patterns_checked_but_never_a_token() {
        let creds = Credentials::parse(
            r#"
            [[credential]]
            match = "github.com/myname/*"
            token = "ghp_very_secret"
            "#,
        )
        .unwrap();
        let err = creds.find("gitlab.com", "group/x").unwrap_err();
        let message = err.to_string();
        assert!(message.contains("gitlab.com/group/x"), "{message}");
        assert!(message.contains("github.com/myname/*"), "{message}");
        assert!(!message.contains("ghp_very_secret"), "{message}");
    }

    #[test]
    fn empty_rule_set_gives_a_clear_message() {
        let err = Credentials::default()
            .find("github.com", "a/b")
            .unwrap_err();
        assert!(err.to_string().contains("has no rules"));
    }

    #[test]
    fn rules_without_host_or_token_are_rejected() {
        let no_host = Credentials::parse(
            r#"
            [[credential]]
            match = "team-a/*"
            token = "t"
            "#,
        );
        assert!(matches!(
            no_host,
            Err(CredentialsError::Invalid { index: 0, .. })
        ));

        let empty_token = Credentials::parse(
            r#"
            [[credential]]
            match = "github.com/*"
            token = " "
            "#,
        );
        assert!(matches!(
            empty_token,
            Err(CredentialsError::Invalid { index: 0, .. })
        ));
    }

    #[test]
    fn debug_output_never_contains_the_token() {
        let creds = Credentials::parse(SAMPLE).unwrap();
        let debug = format!("{creds:?}");
        assert!(!debug.contains("token-single"), "{debug}");
        assert!(debug.contains("Secret(***)"));
    }

    #[test]
    fn glob_star_matches_across_slashes_and_handles_backtracking() {
        assert!(glob_match("a/*", "a/b/c"));
        assert!(glob_match("a/*/c", "a/x/y/c"));
        assert!(glob_match("*b*c", "abxbc"));
        assert!(!glob_match("a/*/c", "a/x/y/d"));
        assert!(!glob_match("a/b", "a/bc"));
        assert!(glob_match("a/b*", "a/b"));
    }

    #[cfg(unix)]
    #[test]
    fn group_or_world_readable_file_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("fleetify-cred-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("credentials.toml");
        fs::write(&path, SAMPLE).unwrap();

        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            Credentials::load(&path),
            Err(CredentialsError::TooOpen { .. })
        ));

        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let (creds, warnings) = Credentials::load(&path).unwrap();
        assert_eq!(creds.rules().len(), 3);
        assert!(warnings.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
