//! Run folders, run ids and the dry-run flow (PLAN.md, "Run flow per repo",
//! steps 1–3 and the "Dry run" acceptance). A dry run refreshes the cache clones,
//! evaluates `applies_to` and reports which credential rule applies. It creates no
//! workspace and pushes nothing.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::clock::rfc3339_now;
use crate::config::{Config, ConfigError, HostTable};
use crate::credentials::{Credentials, CredentialsError};
use crate::events::{Event, EventBus, Redactor};
use crate::git::{Auth, GitContext, GitError, username_for};
use crate::hosts::{Access, Host, TlsCheck, api_base, client_for, tls_self_test};
use crate::recipe::{Recipe, RepoFacts};
use crate::report::{self, Failure, RepoResult, Report};
use crate::repos::{Repo, parse_repos};
use crate::runner::{Runner, interrupt_requested};
use crate::state::{output_path, spawn_recorder, write_atomic};
use crate::workspace::{cache_dir, ensure_cache};

/// Timeout for one git call (clone, fetch, ls-remote).
const GIT_TIMEOUT: Duration = Duration::from_secs(600);

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

    pub fn results_path(&self) -> PathBuf {
        self.root.join("results.jsonl")
    }

    pub fn report_path(&self) -> PathBuf {
        self.root.join("report.json")
    }

    pub fn meta_path(&self) -> PathBuf {
        self.root.join("run.json")
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

/// Run-level facts kept in `run.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunMeta {
    pub run_id: String,
    pub recipe: String,
    pub started: String,
    /// Empty until the run has finished.
    pub finished: String,
}

/// The run folder that was modified most recently, if any.
pub fn latest_run(workdir: &Path) -> io::Result<Option<String>> {
    let runs = workdir.join("runs");
    if !runs.is_dir() {
        return Ok(None);
    }
    let mut newest: Option<(std::time::SystemTime, String)> = None;
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

/// The report of a run: `report.json` if written, otherwise rebuilt from the
/// results log of a run that did not finish.
pub fn load_report(layout: &RunLayout) -> Result<Report, String> {
    if let Ok(text) = fs::read_to_string(layout.report_path()) {
        return serde_json::from_str(&text).map_err(|err| format!("cannot read report: {err}"));
    }
    let meta: RunMeta = fs::read_to_string(layout.meta_path())
        .map_err(|err| format!("run {} has no metadata: {err}", layout.run_id))
        .and_then(|text| serde_json::from_str(&text).map_err(|err| err.to_string()))?;
    let results = report::read_results(&layout.results_path()).map_err(|err| err.to_string())?;
    let redactor = Redactor::default();
    Ok(report::build(
        &meta.run_id,
        &meta.recipe,
        &meta.started,
        &meta.finished,
        results,
        &redactor,
    ))
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

/// Runs the dry run and writes `run.json`, `results.jsonl`, `events.jsonl`,
/// per-repo state and `report.json`. Refuses before doing any work when the
/// configuration, recipe or repos file is wrong.
pub fn dry_run(input: &DryRunInput<'_>) -> Result<Report, Refused> {
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
    let clients = host_clients(&repos)?;

    let layout = input.layout;
    fs::create_dir_all(&layout.root).map_err(|err| Refused(err.to_string()))?;
    let started = rfc3339_now();
    let mut meta = RunMeta {
        run_id: layout.run_id.clone(),
        recipe: input.recipe.name.clone(),
        started: started.clone(),
        finished: String::new(),
    };
    write_meta(layout, &meta).map_err(|err| Refused(err.to_string()))?;

    let bus = EventBus::new();
    let recorder = spawn_recorder(bus.subscribe(), layout.clone())
        .map_err(|err| Refused(format!("cannot write events log: {err}")))?;
    let runner = Runner::new(bus.clone(), redactor.clone());

    let mut results = Vec::with_capacity(repos.len());
    for repo in &repos {
        let result = if interrupt_requested() {
            interrupted_before_start(repo)
        } else {
            dry_run_repo(input, &runner, &credentials, &clients, repo)
        };
        let _ = report::append_result(&layout.results_path(), &result);
        runner.bus().emit(Event::RepoFinished {
            repo: result.repo.clone(),
            result: result.result.clone(),
            message: redactor.redact(&result.detail),
        });
        results.push(result);
    }

    drop(runner);
    drop(bus);
    let _ = recorder.join();

    meta.finished = rfc3339_now();
    write_meta(layout, &meta).map_err(|err| Refused(err.to_string()))?;
    let report = report::build(
        &meta.run_id,
        &meta.recipe,
        &meta.started,
        &meta.finished,
        results,
        &redactor,
    );
    let bytes = serde_json::to_vec_pretty(&report).map_err(|err| Refused(err.to_string()))?;
    write_atomic(&layout.report_path(), &bytes).map_err(|err| Refused(err.to_string()))?;
    Ok(report)
}

/// One API client per host used by the repos, after a TLS self-test per host. A
/// certificate the OS does not trust stops the run: the internal CA is missing.
/// Other connection failures are reported and the run goes on, because the
/// repo itself will then fail with a clearer message.
fn host_clients(repos: &[Repo]) -> Result<BTreeMap<String, Box<dyn Host>>, Refused> {
    let mut clients: BTreeMap<String, Box<dyn Host>> = BTreeMap::new();
    for repo in repos {
        if clients.contains_key(&repo.host) {
            continue;
        }
        let base = api_base(repo.host_type, &repo.host);
        match tls_self_test(&base) {
            TlsCheck::Ok => {}
            TlsCheck::UntrustedCertificate(detail) => {
                return Err(Refused(format!(
                    "TLS: {base} presents a certificate the OS does not trust ({detail}). \
                     Install the internal CA in the operating system's certificate store and retry."
                )));
            }
            TlsCheck::Unreachable(detail) => {
                tracing::warn!(%base, %detail, "TLS self-test could not reach the host");
                eprintln!("warning: cannot reach {base} for the TLS check: {detail}");
            }
        }
        let client = client_for(repo.host_type, &repo.host, None)
            .map_err(|err| Refused(format!("cannot set up the client for {}: {err}", repo.host)))?;
        clients.insert(repo.host.clone(), client);
    }
    Ok(clients)
}

fn write_meta(layout: &RunLayout, meta: &RunMeta) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(meta).map_err(io::Error::other)?;
    write_atomic(&layout.meta_path(), &bytes)
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
    let selected =
        |r: &Repo, suffix: &str| r.path == suffix || r.path.ends_with(&format!("/{suffix}"));
    for suffix in only {
        if !repos.iter().any(|r| selected(r, suffix)) {
            return Err(Refused(format!(
                "--only {suffix}: no repo in {} has this path",
                path.display()
            )));
        }
    }
    Ok(repos
        .into_iter()
        .filter(|r| only.iter().any(|s| selected(r, s)))
        .collect())
}

fn artifacts(layout: &RunLayout, repo: &Repo) -> BTreeMap<String, String> {
    let key = repo.key();
    BTreeMap::from([
        (
            "events".to_string(),
            layout.events_path().display().to_string(),
        ),
        ("log".to_string(), layout.log_path().display().to_string()),
        (
            "output".to_string(),
            output_path(layout, &key).display().to_string(),
        ),
        (
            "repo_dir".to_string(),
            layout.repo_dir(repo).display().to_string(),
        ),
    ])
}

fn interrupted_repo(
    key: &str,
    message: String,
    output_tail: Vec<String>,
    artifacts: BTreeMap<String, String>,
) -> RepoResult {
    let mut result = RepoResult::new(key, "interrupted", &message);
    result.failure = Some(Failure {
        class: "interrupted".to_string(),
        message,
        rule: None,
        output_tail,
        artifacts,
    });
    result
}

fn interrupted_before_start(repo: &Repo) -> RepoResult {
    let mut result = RepoResult::new(
        &repo.key(),
        "interrupted",
        "not started: the run was interrupted",
    );
    result.failure = Some(Failure {
        class: "interrupted".to_string(),
        message: "not started: the run was interrupted".to_string(),
        rule: None,
        output_tail: Vec::new(),
        artifacts: BTreeMap::new(),
    });
    result
}

fn dry_run_repo(
    input: &DryRunInput<'_>,
    runner: &Runner,
    credentials: &Credentials,
    clients: &BTreeMap<String, Box<dyn Host>>,
    repo: &Repo,
) -> RepoResult {
    let key = repo.key();
    let phase = |name: &str| {
        runner.bus().emit(Event::RepoPhase {
            repo: key.clone(),
            phase: name.to_string(),
        })
    };
    phase("cloning");

    let failed = |message: String, rule: Option<String>, tail: Vec<String>| {
        let mut result = RepoResult::new(&key, "error", &message);
        result.rule = rule.clone();
        result.failure = Some(Failure {
            class: "error".to_string(),
            message,
            rule,
            output_tail: tail,
            artifacts: artifacts(input.layout, repo),
        });
        result
    };

    let rule = match credentials.find_rule(&repo.host, &repo.path) {
        Ok(rule) => rule,
        Err(err) => return failed(err.to_string(), None, Vec::new()),
    };
    let pattern = rule.pattern().to_string();
    let host_type = repo.host_type.as_str();

    // Read-access probe first: a rejected token fails here, without a clone.
    let access = match clients
        .get(&repo.host)
        .map(|client| client.probe_read_access(repo, rule.token()))
    {
        Some(Ok(Access::Ok)) => None,
        Some(Ok(denied)) => Some(denied.describe().to_string()),
        Some(Err(err)) => Some(err.to_string()),
        None => Some(format!("no API client for {}", repo.host)),
    };
    if let Some(message) = access {
        return failed(message, Some(pattern), Vec::new());
    }

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
        Err(GitError {
            message,
            output_tail,
        }) => {
            if interrupt_requested() {
                return interrupted_repo(&key, message, output_tail, artifacts(input.layout, repo));
            }
            return failed(message, Some(pattern), output_tail);
        }
    };

    phase("checking");
    let facts = RepoFacts {
        host_type,
        path: &repo.path,
        checkout: &cache,
    };
    let unmet = input.recipe.unmet_conditions(&facts);
    let mut result = if unmet.is_empty() {
        RepoResult::new(
            &key,
            "dry_run",
            &format!(
                "read access ok; would branch {} from {} at {}",
                input.recipe.branch,
                state.default_branch,
                &state.base_sha[..state.base_sha.len().min(12)]
            ),
        )
    } else {
        RepoResult::new(&key, "skipped", &unmet.join("; "))
    };
    result.rule = Some(pattern);
    phase("finished");
    result
}

#[cfg(test)]
mod tests {
    use super::*;

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
