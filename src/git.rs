//! Git commands, all through the command runner. Credentials reach git only via
//! a `GIT_ASKPASS` helper: a `0600` file, written for one call and deleted right
//! after it, read by this same binary (see [`askpass_helper`]). Git's own
//! credential helpers are switched off so no other token source can apply.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::credentials::Secret;
use crate::events::Outcome;
use crate::runner::{BASE_ENV, EnvPolicy, RunRequest, Runner};

/// Environment variable that switches the binary into askpass mode.
pub const ASKPASS_ENV: &str = "FLEETIFY_ASKPASS_FILE";

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

/// Who to authenticate as. The username depends on the host type.
#[derive(Clone, Copy)]
pub struct Auth<'a> {
    pub username: &'a str,
    pub token: &'a Secret,
}

/// Username for HTTPS token authentication, by host type.
pub fn username_for(host_type: &str) -> &'static str {
    match host_type {
        "gitlab" => "oauth2",
        _ => "x-access-token",
    }
}

/// Everything a git call needs besides its arguments.
#[derive(Clone, Copy)]
pub struct GitContext<'a> {
    pub runner: &'a Runner,
    /// Shown in events, usually the repo key.
    pub label: &'a str,
    /// Directory for askpass files. Must not be inside a workspace.
    pub scratch: &'a Path,
    pub auth: Option<Auth<'a>>,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitError {
    pub message: String,
    /// Last standard error lines of the failed command, for the report.
    pub output_tail: Vec<String>,
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for GitError {}

/// Runs `git <args>` in `dir` and returns its standard output.
pub fn git(ctx: &GitContext<'_>, dir: Option<&Path>, args: &[&str]) -> Result<String, GitError> {
    let askpass = match ctx.auth {
        Some(auth) => Some(AskpassFile::create(ctx.scratch, auth)?),
        None => None,
    };

    let mut argv = vec![
        "git".to_string(),
        "-c".to_string(),
        "credential.helper=".to_string(),
    ];
    argv.extend(args.iter().map(|a| a.to_string()));
    let mut request = RunRequest::new(ctx.label, argv);
    request.cwd = dir.map(Path::to_path_buf);
    request.timeout = Some(ctx.timeout);

    let names: Vec<String> = BASE_ENV.iter().map(|s| s.to_string()).collect();
    let mut set = vec![("GIT_TERMINAL_PROMPT".to_string(), "0".to_string())];
    if let Some(file) = &askpass {
        let helper = std::env::current_exe().map_err(|err| GitError {
            message: format!("cannot locate fleetify for askpass: {err}"),
            output_tail: Vec::new(),
        })?;
        set.push(("GIT_ASKPASS".to_string(), helper.display().to_string()));
        set.push((ASKPASS_ENV.to_string(), file.path.display().to_string()));
    }
    request.env = EnvPolicy::Allowlist { names, set };

    let result = ctx.runner.run(request);
    if result.outcome == (Outcome::Exited { code: Some(0) }) {
        return Ok(result.stdout());
    }
    let subcommand = args.first().copied().unwrap_or("");
    let reason = match &result.outcome {
        Outcome::Exited { code: Some(code) } => format!("exit code {code}"),
        Outcome::Exited { code: None } => "killed by a signal".to_string(),
        Outcome::TimedOut => "timed out".to_string(),
        Outcome::Interrupted => "interrupted".to_string(),
        Outcome::WaitingForInput => "waited for input".to_string(),
        Outcome::SpawnFailed { message } | Outcome::Error { message } => message.clone(),
    };
    let tail = result.stderr_tail(5);
    let detail = if tail.is_empty() {
        String::new()
    } else {
        format!(": {tail}")
    };
    Err(GitError {
        message: format!("git {subcommand} failed ({reason}){detail}"),
        output_tail: result.stderr_lines(20),
    })
}

/// A file holding the credentials for one git call. Deleted on drop.
struct AskpassFile {
    path: PathBuf,
}

impl AskpassFile {
    fn create(dir: &Path, auth: Auth<'_>) -> Result<Self, GitError> {
        let io_error = |err: std::io::Error| GitError {
            message: format!("cannot write askpass file in {}: {err}", dir.display()),
            output_tail: Vec::new(),
        };
        fs::create_dir_all(dir).map_err(io_error)?;
        let name = format!(
            "askpass-{}-{}",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        );
        let path = dir.join(name);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path).map_err(io_error)?;
        // Written before the guard exists; a failed write removes the file here.
        if let Err(err) = writeln!(file, "{}\n{}", auth.username, auth.token.expose()) {
            let _ = fs::remove_file(&path);
            return Err(io_error(err));
        }
        Ok(AskpassFile { path })
    }
}

impl Drop for AskpassFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Answers a git credential prompt, if this process was started as an askpass
/// helper (the askpass variable is set). Returns `None` for normal invocations.
pub fn askpass_helper() -> Option<ExitCode> {
    let path = std::env::var_os(ASKPASS_ENV)?;
    let prompt = std::env::args().nth(1)?;
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => return Some(ExitCode::from(1)),
    };
    let mut lines = content.lines();
    let username = lines.next().unwrap_or("");
    let token = lines.next().unwrap_or("");
    println!("{}", askpass_answer(&prompt, username, token));
    Some(ExitCode::SUCCESS)
}

/// The answer to one prompt: the username when git asks for it, otherwise the token.
pub fn askpass_answer<'a>(prompt: &str, username: &'a str, token: &'a str) -> &'a str {
    if prompt.to_ascii_lowercase().contains("username") {
        username
    } else {
        token
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn askpass_answers_username_prompts_with_the_username() {
        assert_eq!(
            askpass_answer("Username for 'https://x': ", "oauth2", "tok"),
            "oauth2"
        );
        assert_eq!(
            askpass_answer("Password for 'https://x': ", "oauth2", "tok"),
            "tok"
        );
    }

    #[test]
    fn username_depends_on_host_type() {
        assert_eq!(username_for("github"), "x-access-token");
        assert_eq!(username_for("gitlab"), "oauth2");
    }

    #[test]
    fn askpass_file_is_removed_on_drop() {
        let dir =
            std::env::temp_dir().join(format!("fleetify-askpass-unit-{}", std::process::id()));
        let secret = Secret::new("tok-unit");
        let auth = Auth {
            username: "x-access-token",
            token: &secret,
        };
        let path = {
            let file = AskpassFile::create(&dir, auth).unwrap();
            let content = fs::read_to_string(&file.path).unwrap();
            assert_eq!(content, "x-access-token\ntok-unit\n");
            file.path.clone()
        };
        assert!(!path.exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
