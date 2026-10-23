//! Repository list parsing and the `Repo` model (milestone M2).
//!
//! ```text
//! github:owner/name                 (github.com)
//! gitlab:group/sub/name             (gitlab.com, subgroups allowed)
//! https://host/path/name.git        (any declared host)
//! ssh://git@host[:port]/path.git
//! git@host:path/name.git            (scp-like SSH)
//! ```
//!
//! A bare `owner/name` is rejected: the host would be ambiguous. The host comes from the
//! URL. A host other than `github.com` and `gitlab.com` must be declared in the recipe
//! (`[hosts."<name>"] type = "github"|"gitlab"`).

use std::collections::BTreeSet;
use std::fmt;

use crate::hosts::{HostTable, HostType};

/// One repository from `repos.txt`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    /// Host name, lowercase, without port.
    pub host: String,
    pub host_type: HostType,
    /// Full path without the host, including subgroups. Never starts or ends with `/`.
    pub path: String,
    /// Last path segment.
    pub name: String,
    /// URL to clone from. For shorthand forms this is HTTPS; for URL forms it is the URL as written.
    pub clone_url: String,
    /// 1-based line in `repos.txt`.
    pub line: usize,
}

impl Repo {
    /// `<host>/<path>`, the key credential rules match against.
    pub fn key(&self) -> String {
        format!("{}/{}", self.host, self.path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoLineError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for RepoLineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for RepoLineError {}

/// Parses the whole file. Every bad line is reported, not just the first.
pub fn parse_repos(text: &str, hosts: &HostTable) -> Result<Vec<Repo>, Vec<RepoLineError>> {
    let mut repos = Vec::new();
    let mut errors = Vec::new();
    let mut seen = BTreeSet::new();

    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        let entry = raw.trim();
        if entry.is_empty() || entry.starts_with('#') {
            continue;
        }
        match parse_entry(entry, hosts) {
            Ok(mut repo) => {
                repo.line = line;
                // Workspace folders mirror host and path, so duplicates would collide.
                if !seen.insert(repo.key().to_ascii_lowercase()) {
                    errors.push(RepoLineError {
                        line,
                        message: format!("duplicate repository {}", repo.key()),
                    });
                } else {
                    repos.push(repo);
                }
            }
            Err(message) => errors.push(RepoLineError { line, message }),
        }
    }

    if errors.is_empty() {
        Ok(repos)
    } else {
        Err(errors)
    }
}

fn parse_entry(entry: &str, hosts: &HostTable) -> Result<Repo, String> {
    if let Some(rest) = entry.strip_prefix("github:") {
        let path = parse_path(rest)?;
        if path.split('/').count() != 2 {
            return Err(format!("github repositories are owner/name, got {path:?}"));
        }
        return shorthand("github.com", path, hosts);
    }
    if let Some(rest) = entry.strip_prefix("gitlab:") {
        let path = parse_path(rest)?;
        if !path.contains('/') {
            return Err(format!("gitlab repositories need group/name, got {path:?}"));
        }
        return shorthand("gitlab.com", path, hosts);
    }
    if let Some((scheme, rest)) = entry.split_once("://") {
        return url_form(entry, scheme, rest, hosts);
    }
    if entry.contains('@') && entry.contains(':') {
        return scp_form(entry, hosts);
    }
    if entry.contains('/') {
        return Err(format!(
            "{entry:?} is ambiguous (which host?); use github:{entry}, gitlab:{entry}, or a clone URL"
        ));
    }
    Err(format!(
        "{entry:?} is not a repository; expected github:owner/name, gitlab:group/name, or a clone URL"
    ))
}

fn shorthand(host: &str, path: String, hosts: &HostTable) -> Result<Repo, String> {
    let host_type = hosts
        .get(host)
        .ok_or_else(|| format!("host {host} is not declared in the recipe"))?;
    let clone_url = format!("https://{host}/{path}.git");
    Ok(build(host, host_type, path, clone_url))
}

fn url_form(entry: &str, scheme: &str, rest: &str, hosts: &HostTable) -> Result<Repo, String> {
    if !scheme.eq_ignore_ascii_case("https") && !scheme.eq_ignore_ascii_case("ssh") {
        return Err(format!(
            "unsupported URL scheme {scheme:?}; use https or ssh"
        ));
    }
    let (authority, path) = rest
        .split_once('/')
        .ok_or_else(|| "clone URL has no repository path".to_string())?;
    if scheme.eq_ignore_ascii_case("https") && authority.contains('@') {
        // Never accept embedded credentials; the message must not echo them.
        return Err("clone URL contains credentials; put tokens in credentials.toml".to_string());
    }
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = host_name(host_port)?;
    let path = parse_path(path)?;
    let host_type = lookup(hosts, &host)?;
    let clone_url = entry.to_string();
    Ok(build(&host, host_type, path, clone_url))
}

fn scp_form(entry: &str, hosts: &HostTable) -> Result<Repo, String> {
    let (user_host, path) = entry
        .split_once(':')
        .ok_or_else(|| "scp-style entry needs user@host:path".to_string())?;
    let (_, host) = user_host
        .split_once('@')
        .ok_or_else(|| "scp-style entry needs user@host:path".to_string())?;
    let host = host_name(host)?;
    let path = parse_path(path)?;
    let host_type = lookup(hosts, &host)?;
    Ok(build(&host, host_type, path, entry.to_string()))
}

fn lookup(hosts: &HostTable, host: &str) -> Result<HostType, String> {
    hosts.get(host).ok_or_else(|| {
        format!("host {host} is not declared in the recipe; add [hosts.\"{host}\"] with type = \"github\" or \"gitlab\" to recipe.toml")
    })
}

fn host_name(raw: &str) -> Result<String, String> {
    if raw.is_empty() || raw.contains('[') {
        return Err(format!("unsupported host {raw:?}"));
    }
    let host = raw.split(':').next().unwrap_or("").to_ascii_lowercase();
    if host.is_empty()
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    {
        return Err(format!("unsupported host {raw:?}"));
    }
    Ok(host)
}

/// Normalises a repository path: trims slashes, drops a trailing `.git`, and
/// rejects empty, `.` and `..` segments and characters outside `[A-Za-z0-9._-]`.
fn parse_path(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim().trim_matches('/');
    let trimmed = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    if trimmed.is_empty() {
        return Err("repository path is empty".to_string());
    }
    for segment in trimmed.split('/') {
        let valid = !segment.is_empty()
            && segment != "."
            && segment != ".."
            && segment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
        if !valid {
            return Err(format!("invalid repository path {trimmed:?}"));
        }
    }
    Ok(trimmed.to_string())
}

fn build(host: &str, host_type: HostType, path: String, clone_url: String) -> Repo {
    let name = path.rsplit('/').next().unwrap_or(&path).to_string();
    Repo {
        host: host.to_string(),
        host_type,
        path,
        name,
        clone_url,
        line: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::Recipe;

    fn hosts() -> HostTable {
        Recipe::parse(
            r#"
            schema = 1
            name = "hosts"
            branch = "b"
            pr_title = "t"
            [hosts."gitlab.example.com"]
            type = "gitlab"
            "#,
        )
        .unwrap()
        .host_table()
        .unwrap()
    }

    fn one(line: &str) -> Repo {
        let mut repos = parse_repos(line, &hosts()).unwrap();
        assert_eq!(repos.len(), 1);
        repos.remove(0)
    }

    fn errors(text: &str) -> Vec<RepoLineError> {
        parse_repos(text, &hosts()).unwrap_err()
    }

    #[test]
    fn github_shorthand() {
        let repo = one("github:myname/project-a");
        assert_eq!(repo.host, "github.com");
        assert_eq!(repo.host_type, HostType::Github);
        assert_eq!(repo.path, "myname/project-a");
        assert_eq!(repo.name, "project-a");
        assert_eq!(repo.clone_url, "https://github.com/myname/project-a.git");
        assert_eq!(repo.line, 1);
    }

    #[test]
    fn gitlab_shorthand_allows_subgroups() {
        let repo = one("gitlab:mygroup/subgroup/project-c");
        assert_eq!(repo.host, "gitlab.com");
        assert_eq!(repo.path, "mygroup/subgroup/project-c");
        assert_eq!(repo.name, "project-c");
        assert_eq!(
            repo.clone_url,
            "https://gitlab.com/mygroup/subgroup/project-c.git"
        );
    }

    #[test]
    fn scp_style_ssh_url() {
        let repo = one("git@github.com:myname/project-b.git");
        assert_eq!(repo.host, "github.com");
        assert_eq!(repo.path, "myname/project-b");
        assert_eq!(repo.clone_url, "git@github.com:myname/project-b.git");
    }

    #[test]
    fn https_url_on_configured_self_hosted_host() {
        let repo = one("https://gitlab.example.com/team/project-d.git");
        assert_eq!(repo.host, "gitlab.example.com");
        assert_eq!(repo.host_type, HostType::Gitlab);
        assert_eq!(repo.path, "team/project-d");
        assert_eq!(
            repo.clone_url,
            "https://gitlab.example.com/team/project-d.git"
        );
    }

    #[test]
    fn ssh_url_with_port_keeps_port_only_in_clone_url() {
        let repo = one("ssh://git@gitlab.example.com:2222/team/sub/project-d.git");
        assert_eq!(repo.host, "gitlab.example.com");
        assert_eq!(repo.path, "team/sub/project-d");
        assert_eq!(
            repo.clone_url,
            "ssh://git@gitlab.example.com:2222/team/sub/project-d.git"
        );
    }

    #[test]
    fn comments_and_blank_lines_are_skipped_and_line_numbers_kept() {
        let repos =
            parse_repos("# header\n\n  github:a/b  \n# note\ngithub:a/c\n", &hosts()).unwrap();
        assert_eq!(repos.len(), 2);
        assert_eq!(repos[0].line, 3);
        assert_eq!(repos[1].line, 5);
    }

    #[test]
    fn bare_owner_name_is_rejected_as_ambiguous() {
        let errs = errors("myname/project-a");
        assert_eq!(errs.len(), 1);
        assert!(errs[0].message.contains("ambiguous"), "{}", errs[0].message);
    }

    #[test]
    fn unknown_host_names_the_recipe_entry_to_add() {
        let errs = errors("https://git.unknown.example/team/x.git");
        assert!(
            errs[0].message.contains("[hosts.\"git.unknown.example\"]"),
            "{}",
            errs[0].message
        );
    }

    #[test]
    fn path_traversal_and_odd_characters_are_rejected() {
        for bad in [
            "github:owner/..",
            "github:../name",
            "github:owner/a b",
            "github:owner/a/b",
            "gitlab:group/./x",
            "https://gitlab.example.com/team/../x.git",
        ] {
            assert_eq!(errors(bad).len(), 1, "{bad} should be rejected");
        }
    }

    #[test]
    fn embedded_credentials_in_https_url_are_rejected_without_echo() {
        let errs = errors("https://user:glpat-secret@gitlab.example.com/team/x.git");
        assert_eq!(errs.len(), 1);
        assert!(!errs[0].message.contains("glpat-secret"));
    }

    #[test]
    fn unsupported_scheme_is_rejected() {
        let errs = errors("http://gitlab.example.com/team/x.git");
        assert!(errs[0].message.contains("unsupported URL scheme"));
    }

    #[test]
    fn github_shorthand_needs_exactly_owner_and_name() {
        assert_eq!(errors("github:owner").len(), 1);
        assert_eq!(errors("github:a/b/c").len(), 1);
    }

    #[test]
    fn duplicates_are_reported_case_insensitively() {
        let errs = errors("github:Owner/Repo\ngithub:owner/repo\n");
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].line, 2);
        assert!(errs[0].message.contains("duplicate"));
    }

    #[test]
    fn every_bad_line_is_reported() {
        let errs = errors("github:ok/one\nnope\ngithub:a/b/c\n");
        let lines: Vec<usize> = errs.iter().map(|e| e.line).collect();
        assert_eq!(lines, vec![2, 3]);
    }

    #[test]
    fn repo_key_is_host_slash_path() {
        assert_eq!(one("github:a/b").key(), "github.com/a/b");
    }
}
