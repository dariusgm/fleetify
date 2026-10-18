//! Run folders, run ids and the dry-run flow (PLAN.md, "Run flow per repo",
//! steps 1–3 and the "Dry run" acceptance). A dry run refreshes the cache clones,
//! evaluates `applies_to` and reports which credential rule applies. It creates no
//! workspace and pushes nothing.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::thread::{self, JoinHandle};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::{Config, ConfigError, HostTable};
use crate::credentials::{Credentials, CredentialsError};
use crate::events::{Event, EventBus, Redactor};
use crate::git::{Auth, GitContext, username_for};
use crate::recipe::{Recipe, RepoFacts};
use crate::repos::{Repo, parse_repos};
use crate::runner::Runner;
use crate::workspace::{cache_dir, ensure_cache};

use std::time::Duration;

/// Timeout for one git call (clone, fetch, ls-remote).
const GIT_TIMEOUT: Duration = Duration::from_secs(600);

/// Today's date in UTC as `YYYY-MM-DD`.
pub fn today_utc() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Converts days since 1970-01-01 to a (year, month, day) date.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Run id: the given name, or `<recipe>-<YYYY-MM-DD>`.
pub fn run_id(recipe_name: &str, requested: Option<&str>, date: &str) -> String {
    match requested {
        Some(name) => name.to_string(),
        None => format!("{recipe_name}-{date}"),
    }
}

/// Paths of one run and of the shared working directory.
#[derive(Debug, Clone)]
pub struct RunLayout {
    pub workdir: PathBuf,
    pub run_id: String,
    pub root: PathBuf,
}

impl RunLayout {
    pub fn new(workdir: &Path, run_id: &str) -> Self {
        RunLayout {
            workdir: workdir.to_path_buf(),
            run_id: run_id.to_string(),
            root: workdir.join("runs").join(run_id),
        }
    }

    pub fn log_path(&self) -> PathBuf {
        self.root.join("fleetify.log")
    }

    pub fn events_path(&self) -> PathBuf {
        self.root.join("events.jsonl")
    }

    /// Folder of one repo inside the run: `<host>/<path>`.
    pub fn repo_dir(&self, repo: &Repo) -> PathBuf {
        self.root.join(&repo.host).join(&repo.path)
    }

    pub fn workspace_dir(&self, repo: &Repo) -> PathBuf {
        self.repo_dir(repo).join("workspace")
    }

    /// Scratch space for askpass files, outside every workspace.
    pub fn scratch_dir(&self) -> PathBuf {
        self.workdir.join("tmp")
    }
}

/// The run folder that was modified most recently, if any.
pub fn latest_run(workdir: &Path) -> io::Result<Option<String>> {
    let runs = workdir.join("runs");
    if !runs.is_dir() {
        return Ok(None);
    }
    let mut newest: Option<(SystemTime, String)> = None;
    for entry in fs::read_dir(runs)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let modified = entry.metadata()?.modified()?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if newest.as_ref().is_none_or(|(time, _)| modified > *time) {
            newest = Some((modified, name));
        }
    }
    Ok(newest.map(|(_, name)| name))
}

/// Result of one repo in a dry run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub repo: String,
    /// `dry_run`, `skipped` or `error`.
    pub result: &'static str,
    /// The credential rule that applies (its pattern, never its token).
    pub rule: Option<String>,
    pub detail: String,
}

impl Row {
    /// Only `error` is unsuccessful in a dry run.
    pub fn is_success(&self) -> bool {
        self.result != "error"
    }
}

/// Why a run cannot start at all (exit code 3).
#[derive(Debug)]
pub struct Refused(pub String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<ConfigError> for Refused {
    fn from(err: ConfigError) -> Self {
        Refused(err.to_string())
    }
}

impl From<CredentialsError> for Refused {
    fn from(err: CredentialsError) -> Self {
        Refused(err.to_string())
    }
}

/// Everything a dry run needs.
pub struct DryRunInput<'a> {
    pub workdir: &'a Path,
    pub recipe_dir: &'a Path,
    pub recipe: &'a Recipe,
    pub repos_file: &'a Path,
    pub layout: &'a RunLayout,
    pub only: &'a [String],
    pub agent: Option<&'a str>,
}

/// Runs the dry run. Returns one row per repo, or refuses before doing any work.
pub fn dry_run(input: &DryRunInput<'_>) -> Result<Vec<Row>, Refused> {
    let config = Config::load(&input.workdir.join("config.toml"))?;
    let hosts = config.host_table()?;
    let (credentials, warnings) = Credentials::load(&input.workdir.join("credentials.toml"))?;
    for warning in &warnings {
        tracing::warn!(%warning, "credentials");
        eprintln!("warning: {warning}");
    }
    let redactor = Redactor::new(credentials.secrets());
    crate::logging::set_redactor(redactor.clone());

    let issues = input.recipe.validate(input.recipe_dir, input.agent);
    if !issues.is_empty() {
        let list: Vec<String> = issues.iter().map(ToString::to_string).collect();
        return Err(Refused(format!("recipe refused: {}", list.join("; "))));
    }

    let repos = load_repos(input.repos_file, &hosts, input.only)?;

    fs::create_dir_all(input.layout.root.clone()).map_err(|err| Refused(err.to_string()))?;
    let bus = EventBus::new();
    let writer = spawn_event_writer(bus.subscribe(), &input.layout.events_path())
        .map_err(|err| Refused(format!("cannot write events log: {err}")))?;
    let runner = Runner::new(bus.clone(), redactor.clone());

    let rows: Vec<Row> = repos
        .iter()
        .map(|repo| {
            let row = dry_run_repo(input, &runner, &credentials, repo);
            runner.bus().emit(Event::RepoFinished {
                repo: repo.key(),
                result: row.result.to_string(),
                message: redactor.redact(&row.detail),
            });
            row
        })
        .collect();

    drop(runner);
    drop(bus);
    let _ = writer.join();
    Ok(rows)
}

fn load_repos(path: &Path, hosts: &HostTable, only: &[String]) -> Result<Vec<Repo>, Refused> {
    let text = fs::read_to_string(path)
        .map_err(|err| Refused(format!("cannot read {}: {err}", path.display())))?;
    let repos = parse_repos(&text, hosts).map_err(|errors| {
        let list: Vec<String> = errors.iter().map(ToString::to_string).collect();
        Refused(format!("{}: {}", path.display(), list.join("; ")))
    })?;
    if only.is_empty() {
        return Ok(repos);
    }
    for suffix in only {
        let matches = repos
            .iter()
            .any(|r| r.path == *suffix || r.path.ends_with(&format!("/{suffix}")));
        if !matches {
            return Err(Refused(format!(
                "--only {suffix}: no repo in {} has this path",
                path.display()
            )));
        }
    }
    Ok(repos
        .into_iter()
        .filter(|r| {
            only.iter()
                .any(|s| r.path == *s || r.path.ends_with(&format!("/{s}")))
        })
        .collect())
}

fn dry_run_repo(
    input: &DryRunInput<'_>,
    runner: &Runner,
    credentials: &Credentials,
    repo: &Repo,
) -> Row {
    let key = repo.key();
    let failed = |detail: String, rule: Option<String>| Row {
        repo: key.clone(),
        result: "error",
        rule,
        detail,
    };

    let rule = match credentials.find_rule(&repo.host, &repo.path) {
        Ok(rule) => rule,
        Err(err) => return failed(err.to_string(), None),
    };
    let pattern = rule.pattern().to_string();
    let host_type = repo.host_type.as_str();
    let scratch = input.layout.scratch_dir();
    let ctx = GitContext {
        runner,
        label: &key,
        scratch: &scratch,
        auth: Some(Auth {
            username: username_for(host_type),
            token: rule.token(),
        }),
        timeout: GIT_TIMEOUT,
    };

    let cache = cache_dir(input.workdir, &repo.host, &repo.path);
    let state = match ensure_cache(&ctx, &repo.clone_url, &cache) {
        Ok(state) => state,
        Err(err) => return failed(err.message, Some(pattern)),
    };

    let facts = RepoFacts {
        host_type,
        path: &repo.path,
        checkout: &cache,
    };
    let unmet = input.recipe.unmet_conditions(&facts);
    if !unmet.is_empty() {
        return Row {
            repo: key,
            result: "skipped",
            rule: Some(pattern),
            detail: unmet.join("; "),
        };
    }
    Row {
        repo: key,
        result: "dry_run",
        rule: Some(pattern),
        detail: format!(
            "would branch {} from {} at {}",
            input.recipe.branch,
            state.default_branch,
            &state.base_sha[..state.base_sha.len().min(12)]
        ),
    }
}

/// Writes every event as one JSON line until the bus is dropped.
fn spawn_event_writer(events: Receiver<Event>, path: &Path) -> io::Result<JoinHandle<()>> {
    let mut file: File = File::create(path)?;
    Ok(thread::spawn(move || {
        for event in events {
            if let Ok(line) = serde_json::to_string(&event) {
                let _ = writeln!(file, "{line}");
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_are_converted_correctly() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(11_017), (2000, 3, 1));
        assert_eq!(civil_from_days(20_000), (2024, 10, 4));
    }

    #[test]
    fn run_id_defaults_to_recipe_and_date() {
        assert_eq!(
            run_id("enable-gitlab-ci", None, "2026-10-18"),
            "enable-gitlab-ci-2026-10-18"
        );
        assert_eq!(run_id("x", Some("trial"), "2026-10-18"), "trial");
    }

    #[test]
    fn repo_folders_mirror_host_and_path() {
        let layout = RunLayout::new(Path::new("/w"), "r1");
        let repo = Repo {
            host: "gitlab.com".into(),
            host_type: crate::config::HostType::Gitlab,
            path: "group/sub/name".into(),
            name: "name".into(),
            clone_url: String::new(),
            line: 1,
        };
        assert_eq!(
            layout.workspace_dir(&repo),
            Path::new("/w/runs/r1/gitlab.com/group/sub/name/workspace")
        );
    }
}
