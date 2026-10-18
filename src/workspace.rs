//! Cache clones and per-run workspaces (PLAN.md, "The four basic questions").
//!
//! A cache clone lives under `clones/<host>/<path>` and always sits on the
//! default branch. A workspace is a `git clone --local` of the cache, so it
//! hard-links objects and has no link back to the cache's working tree. Its
//! `origin` is the real remote, but `remote.origin.pushurl` is set to an invalid
//! value: pushing from inside a workspace fails, and the orchestrator pushes
//! through an explicit URL instead (M7).

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use crate::git::{GitContext, GitError, git};

/// Push URL of every workspace. Not a valid transport, so `git push` fails.
pub const DISABLED_PUSH_URL: &str = "invalid://fleetify-push-disabled";

/// Where the cache clone of a repo lives.
pub fn cache_dir(workdir: &Path, host: &str, path: &str) -> std::path::PathBuf {
    workdir.join("clones").join(host).join(path)
}

/// State of a cache clone after it was refreshed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheState {
    pub default_branch: String,
    /// Commit at the tip of the default branch.
    pub base_sha: String,
}

/// Parses the `ref: refs/heads/<name>\tHEAD` line of `git ls-remote --symref`.
pub fn parse_default_branch(ls_remote_output: &str) -> Option<String> {
    ls_remote_output.lines().find_map(|line| {
        let rest = line.strip_prefix("ref: refs/heads/")?;
        let (branch, name) = rest.split_once('\t')?;
        (name == "HEAD").then(|| branch.to_string())
    })
}

/// Clones the repo into `cache` if missing, otherwise fetches it, then puts the
/// cache on the remote's default branch at the fetched tip.
pub fn ensure_cache(
    ctx: &GitContext<'_>,
    clone_url: &str,
    cache: &Path,
) -> Result<CacheState, GitError> {
    let listing = git(ctx, None, &["ls-remote", "--symref", clone_url, "HEAD"])?;
    let default_branch = parse_default_branch(&listing).ok_or_else(|| GitError {
        message: "remote has no default branch (is the repository empty?)".to_string(),
    })?;

    if cache.join(".git").exists() {
        git(
            ctx,
            Some(cache),
            &["remote", "set-url", "origin", clone_url],
        )?;
        git(ctx, Some(cache), &["fetch", "--prune", "--force", "origin"])?;
    } else {
        if let Some(parent) = cache.parent() {
            fs::create_dir_all(parent).map_err(|err| io_error(cache, err))?;
        }
        let cache_arg = cache.display().to_string();
        git(ctx, None, &["clone", "--quiet", clone_url, &cache_arg])?;
    }

    let remote_ref = format!("refs/remotes/origin/{default_branch}");
    let base_sha = git(
        ctx,
        Some(cache),
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{remote_ref}^{{commit}}"),
        ],
    )?
    .trim()
    .to_string();
    git(
        ctx,
        Some(cache),
        &[
            "checkout",
            "--quiet",
            "--force",
            "-B",
            &default_branch,
            &remote_ref,
        ],
    )?;
    git(ctx, Some(cache), &["clean", "--quiet", "-ffdx"])?;

    Ok(CacheState {
        default_branch,
        base_sha,
    })
}

/// Creates a workspace at `workspace` from the cache, on a new `branch` at `base_sha`.
pub fn create_workspace(
    ctx: &GitContext<'_>,
    cache: &Path,
    clone_url: &str,
    workspace: &Path,
    base_sha: &str,
    branch: &str,
) -> Result<(), GitError> {
    if workspace.exists() {
        return Err(GitError {
            message: format!("workspace {} already exists", workspace.display()),
        });
    }
    if let Some(parent) = workspace.parent() {
        fs::create_dir_all(parent).map_err(|err| io_error(workspace, err))?;
    }
    let cache_arg = cache.display().to_string();
    let workspace_arg = workspace.display().to_string();
    git(
        ctx,
        None,
        &["clone", "--quiet", "--local", &cache_arg, &workspace_arg],
    )?;
    git(
        ctx,
        Some(workspace),
        &["checkout", "--quiet", "-B", branch, base_sha],
    )?;
    git(
        ctx,
        Some(workspace),
        &["remote", "set-url", "origin", clone_url],
    )?;
    git(
        ctx,
        Some(workspace),
        &["config", "remote.origin.pushurl", DISABLED_PUSH_URL],
    )?;
    exclude_agent_dir(workspace).map_err(|err| io_error(workspace, err))?;
    Ok(())
}

/// Keeps `.agent/` (task files, attachments, tools copy) out of commits.
fn exclude_agent_dir(workspace: &Path) -> io::Result<()> {
    let path = workspace.join(".git").join("info").join("exclude");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, ".agent/")
}

/// Deletes every workspace below a run folder. Cache clones and logs are kept.
/// Returns how many workspaces were removed.
pub fn remove_workspaces(run_dir: &Path) -> io::Result<usize> {
    let mut removed = 0;
    for entry in fs::read_dir(run_dir)? {
        let path = entry?.path();
        if !path.is_dir() {
            continue;
        }
        if path.file_name().is_some_and(|name| name == "workspace") && path.join(".git").exists() {
            fs::remove_dir_all(&path)?;
            removed += 1;
        } else {
            removed += remove_workspaces(&path)?;
        }
    }
    Ok(removed)
}

fn io_error(path: &Path, err: io::Error) -> GitError {
    GitError {
        message: format!("{}: {err}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_branch_is_read_from_symref_output() {
        let output = "ref: refs/heads/trunk\tHEAD\nabc123\tHEAD\nabc123\trefs/heads/trunk\n";
        assert_eq!(parse_default_branch(output).as_deref(), Some("trunk"));
        assert_eq!(parse_default_branch(""), None);
    }

    #[test]
    fn cache_dirs_mirror_host_and_path() {
        let dir = cache_dir(Path::new("/w"), "gitlab.com", "group/sub/name");
        assert_eq!(
            dir,
            Path::new("/w")
                .join("clones")
                .join("gitlab.com")
                .join("group/sub/name")
        );
    }
}
