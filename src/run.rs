//! Runs: run folders, the shared setup, the worker pool and the report. A dry run
//! and a real run share everything except the per-repo step (docs/design/run-flow.md, "Run flow
//! per repo"). A dry run only refreshes caches, evaluates `applies_to` and probes
//! read access. Nothing is pushed.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::agents;
use crate::clock::rfc3339_now;
use crate::container;
use crate::credentials::{Credentials, CredentialsError, Secret};
use crate::events::{Event, EventBus, Redactor};
use crate::git::{Auth, GitContext, GitError, username_for};
use crate::grants::Grants;
use crate::hosts::{self, Access, Host, HostTable, TlsCheck, api_base, tls_self_test};
use crate::pipeline;
use crate::recipe::{Recipe, RepoFacts};
use crate::report::{self, Failure, RepoResult, Report};
use crate::repos::{Repo, parse_repos};
use crate::runner::{Runner, interrupt_requested};
use crate::state::{output_path, spawn_recorder, write_atomic};
use crate::workspace::{cache_dir, ensure_cache};

/// Timeout for one git call (clone, fetch, ls-remote).
pub const GIT_TIMEOUT: Duration = Duration::from_secs(600);

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
    Ok(report::build(
        &meta.run_id,
        &meta.recipe,
        &meta.started,
        &meta.finished,
        results,
        &Redactor::default(),
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

impl From<CredentialsError> for Refused {
    fn from(err: CredentialsError) -> Self {
        Refused(err.to_string())
    }
}

/// How a run is cut into waves. `sizes` ends with `None`, which takes all repos that
/// remain. `canaries` replace the first wave when given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WavePlan {
    pub sizes: Vec<Option<usize>>,
    pub canaries: Vec<String>,
}

impl Default for WavePlan {
    fn default() -> Self {
        WavePlan {
            sizes: vec![Some(1), None],
            canaries: Vec::new(),
        }
    }
}

/// The caller's side of a wave boundary. `proceed` gets the results of the wave that
/// just finished and the number of repos still waiting; `true` starts the next wave.
pub struct Waves<'a> {
    pub plan: &'a WavePlan,
    pub proceed: &'a dyn Fn(&[RepoResult], usize) -> bool,
}

/// Parses `--waves`, e.g. `1,all` or `1,5,all`. `all` must be the last wave.
pub fn parse_waves(text: &str) -> Result<Vec<Option<usize>>, String> {
    let tokens: Vec<&str> = text.split(',').map(str::trim).collect();
    let mut sizes = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        if *token == "all" {
            if index + 1 != tokens.len() {
                return Err("\"all\" must be the last wave".to_string());
            }
            sizes.push(None);
        } else {
            match token.parse::<usize>() {
                Ok(size) if size > 0 => sizes.push(Some(size)),
                _ => {
                    return Err(format!(
                        "wave size {token:?} must be a positive number or \"all\""
                    ));
                }
            }
        }
    }
    if sizes.last() != Some(&None) {
        return Err("the last wave must be \"all\", for example \"1,all\"".to_string());
    }
    Ok(sizes)
}

/// Whether a repo's path is `suffix` or ends with `/suffix`. Used by `--only` and canaries.
fn path_matches(repo: &Repo, suffix: &str) -> bool {
    repo.path == suffix || repo.path.ends_with(&format!("/{suffix}"))
}

/// Cuts the repos into waves in list order. Canaries, when given, are the first wave.
pub fn split_waves(repos: &[Repo], plan: &WavePlan) -> Result<Vec<Vec<Repo>>, Refused> {
    let mut rest: Vec<Repo> = repos.to_vec();
    let mut waves: Vec<Vec<Repo>> = Vec::new();
    let mut sizes = plan.sizes.iter().copied();
    if !plan.canaries.is_empty() {
        for suffix in &plan.canaries {
            if !repos.iter().any(|r| path_matches(r, suffix)) {
                return Err(Refused(format!(
                    "--canary-repos {suffix}: no repo has this path"
                )));
            }
        }
        let (canary, others): (Vec<Repo>, Vec<Repo>) = rest
            .into_iter()
            .partition(|r| plan.canaries.iter().any(|s| path_matches(r, s)));
        rest = others;
        waves.push(canary);
        sizes.next();
    }
    for size in sizes {
        if rest.is_empty() {
            break;
        }
        let take = size.unwrap_or(rest.len()).min(rest.len());
        waves.push(rest.drain(..take).collect());
    }
    if !rest.is_empty() {
        waves.push(rest);
    }
    Ok(waves.into_iter().filter(|wave| !wave.is_empty()).collect())
}

/// What a run does per repo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    DryRun,
    /// Full no-agent pipeline; `parallel` repos at a time.
    Pipeline {
        parallel: usize,
    },
}

/// Inputs as given on the command line.
pub struct RunInput<'a> {
    pub workdir: &'a Path,
    /// The credentials file, with the grants (`--credentials`, default
    /// `~/.config/fleetify/credentials.toml`).
    pub credentials: &'a Path,
    pub recipe_dir: &'a Path,
    pub recipe: &'a Recipe,
    pub repos_file: &'a Path,
    pub layout: &'a RunLayout,
    pub only: &'a [String],
    pub agent: Option<&'a str>,
    /// `--model`: the model for the agent, overriding the recipe.
    pub model: Option<&'a str>,
    pub allow_unchecked_agent: bool,
    pub waves: Waves<'a>,
    /// `--merge`: merge after the change request is open, if the recipe asks for it.
    pub merge: bool,
    /// How progress is shown while the run runs.
    pub view: crate::ui::View,
}

/// Everything a run needs after loading and checking the inputs.
pub struct Loaded {
    pub credentials: Credentials,
    pub repos: Vec<Repo>,
    pub clients: BTreeMap<String, Box<dyn Host>>,
    /// The agent that runs the recipe's instructions. `None` for a recipe without instructions.
    pub agent: Option<String>,
    /// The model for that agent. `None` leaves the choice to the agent's configuration.
    pub model: Option<String>,
    pub merge: bool,
}

/// Loads credentials, repos and host clients, and runs the TLS self-test.
pub fn load(input: &RunInput<'_>) -> Result<Loaded, Refused> {
    load_for(input, false)
}

/// Refuses the files of the layouts this binary no longer reads. A file that is ignored
/// silently would change what a run does, so a leftover one stops the run and names the way
/// out.
fn refuse_leftover_files(input: &RunInput<'_>) -> Result<(), Refused> {
    let dir = input.credentials.parent().unwrap_or_else(|| Path::new("."));
    for found in [dir.join("config.toml"), input.workdir.join("config.toml")] {
        if found.exists() {
            return Err(Refused(format!(
                "{} is no longer read. Move the recipe settings into recipe.toml and the grants \
                 (permissions, merge_policy) into credentials.toml, then remove the file.",
                found.display()
            )));
        }
    }
    let old_credentials = input.workdir.join("credentials.toml");
    if !input.credentials.exists() && old_credentials.exists() {
        return Err(Refused(format!(
            "credentials.toml is no longer read from the working directory: found {}, but fleetify reads {}. Move the file there.",
            old_credentials.display(),
            input.credentials.display()
        )));
    }
    Ok(())
}

/// The values of the variables the recipe passes into the agent's container, for redaction.
fn container_env_values(recipe: &Recipe) -> Vec<String> {
    recipe
        .agent_env
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .collect()
}

/// Loads what a run needs. A dry run reports missing grants and keeps going; a real
/// run refuses to start.
fn load_for(input: &RunInput<'_>, dry_run: bool) -> Result<Loaded, Refused> {
    refuse_leftover_files(input)?;
    let hosts = input.recipe.host_table().map_err(Refused)?;
    let (credentials, warnings) = Credentials::load(input.credentials)?;
    for warning in &warnings {
        tracing::warn!(%warning, "credentials");
        eprintln!("warning: {warning}");
    }
    crate::logging::set_redactor(Redactor::new(
        credentials.secrets().chain(
            container_env_values(input.recipe)
                .iter()
                .map(String::as_str),
        ),
    ));
    let (agent, model) = match check_agent(input, &credentials.grants, dry_run)? {
        Some((name, model)) => (Some(name), model),
        None => (None, None),
    };
    if let Some(name) = agent.as_deref()
        && agents::grants(input.recipe, &credentials.grants.permissions, name).unrestricted
    {
        tracing::warn!(agent = name, "agent runs unrestricted");
        eprintln!(
            "WARNING: agent {name} runs UNRESTRICTED (the recipe asks for it and credentials.toml \
             grants it). It is not confined by fleetify's grants. This is your own choice."
        );
    }
    check_merge(input.recipe, input.merge).map_err(Refused)?;
    let repos = load_repos(input.repos_file, &hosts, input.only)?;
    let clients = host_clients(&repos, input.recipe.rate_limit.max_wait())?;
    Ok(Loaded {
        credentials,
        repos,
        clients,
        agent,
        model,
        merge: input.merge,
    })
}

/// `--merge` needs the recipe to ask for a merge. Whether a repository may be merged
/// without CI is decided per repository in the pipeline, from `[[merge_policy]]`.
pub fn check_merge(recipe: &Recipe, merge: bool) -> Result<(), String> {
    if merge && recipe.ci.merge != "when_green" {
        return Err("--merge needs ci.merge = \"when_green\" in the recipe".to_string());
    }
    Ok(())
}

/// Checks what an agent run needs before any repo is touched: the agent is supported,
/// the model is a valid name, the user's grants cover what the recipe asks for, and the agent
/// passed `check-agent` for that model when the recipe's policy requires it. Returns the
/// agent's name and model, or `None` without instructions.
fn check_agent(
    input: &RunInput<'_>,
    grants: &Grants,
    dry_run: bool,
) -> Result<Option<(String, Option<String>)>, Refused> {
    if input.recipe.instructions.is_empty() {
        return Ok(None);
    }
    let name = input
        .agent
        .or(input.recipe.agent.as_deref())
        .ok_or_else(|| Refused("instructions are set but no agent is".into()))?;
    agents::check_supported(name).map_err(Refused)?;
    let model = agents::resolve_model(input.model, input.recipe, name);
    if let Some(model) = &model {
        agents::check_model(model).map_err(Refused)?;
    }
    eprintln!(
        "agent {name}{}: {}",
        model
            .as_ref()
            .map_or_else(String::new, |model| format!(" model {model}")),
        agents::describe_grants(input.recipe, &grants.permissions, name)
    );
    if let Some(image) = &input.recipe.image {
        eprintln!("agent {name} runs in the container image {image}");
        if let Err(message) = container::check_image(image, name) {
            if !dry_run {
                return Err(Refused(message));
            }
            eprintln!("dry run: a real run would refuse: {message}");
        }
    }
    let missing = agents::missing_permissions(input.recipe, &grants.permissions, name);
    if !missing.is_empty() {
        let message = format!(
            "credentials.toml does not grant what the recipe asks for: {}",
            missing.join(", ")
        );
        if !dry_run {
            return Err(Refused(message));
        }
        eprintln!("dry run: a real run would refuse: {message}");
    }
    for name in &input.recipe.agent_env {
        if std::env::var_os(name).is_none() {
            let message = format!(
                "environment variable {name} is not set; the recipe passes it to the agent container"
            );
            if !dry_run {
                return Err(Refused(message));
            }
            eprintln!("dry run: a real run would refuse: {message}");
        }
    }
    if input.recipe.policy.require_checked_agent
        && !input.allow_unchecked_agent
        && !agents::is_checked(
            input.workdir,
            name,
            model.as_deref(),
            input.recipe.image.as_deref(),
        )
    {
        return Err(Refused(format!(
            "agent {name} has not passed check-agent: run `fleetify check-agent {name}` or pass --allow-unchecked-agent"
        )));
    }
    Ok(Some((name.to_string(), model)))
}

/// Runs the whole run: validates the recipe, then processes every repo.
pub fn execute(input: &RunInput<'_>, mode: Mode) -> Result<Report, Refused> {
    let issues = input.recipe.validate(input.recipe_dir, input.agent);
    if !issues.is_empty() {
        let list: Vec<String> = issues.iter().map(ToString::to_string).collect();
        return Err(Refused(format!("recipe refused: {}", list.join("; "))));
    }
    let loaded = load_for(input, matches!(mode, Mode::DryRun))?;
    execute_loaded_with_view(
        input.layout,
        input.recipe,
        input.recipe_dir,
        input.workdir,
        loaded,
        mode,
        &input.waves,
        input.view,
    )
}

/// The part of a run after loading: meta, workers, results, report. Tests call
/// this directly with fake clients and local repositories.
pub fn execute_loaded(
    layout: &RunLayout,
    recipe: &Recipe,
    recipe_dir: &Path,
    workdir: &Path,
    loaded: Loaded,
    mode: Mode,
    waves: &Waves<'_>,
) -> Result<Report, Refused> {
    execute_loaded_with_view(
        layout,
        recipe,
        recipe_dir,
        workdir,
        loaded,
        mode,
        waves,
        crate::ui::View::Quiet,
    )
}

#[allow(clippy::too_many_arguments)]
fn execute_loaded_with_view(
    layout: &RunLayout,
    recipe: &Recipe,
    recipe_dir: &Path,
    workdir: &Path,
    loaded: Loaded,
    mode: Mode,
    waves: &Waves<'_>,
    view: crate::ui::View,
) -> Result<Report, Refused> {
    // A dry run has no waves: it plans every repo once.
    let plan = match mode {
        Mode::DryRun => vec![loaded.repos.clone()],
        Mode::Pipeline { .. } => split_waves(&loaded.repos, waves.plan)?,
    };
    fs::create_dir_all(&layout.root).map_err(|err| Refused(err.to_string()))?;
    let started = rfc3339_now();
    let mut meta = RunMeta {
        run_id: layout.run_id.clone(),
        recipe: recipe.name.clone(),
        started: started.clone(),
        finished: String::new(),
    };
    write_meta(layout, &meta).map_err(|err| Refused(err.to_string()))?;

    let redactor = Redactor::new(
        loaded
            .credentials
            .secrets()
            .chain(container_env_values(recipe).iter().map(String::as_str)),
    );
    let bus = EventBus::new();
    // Rate-limit waits of the host clients go to the same event stream as everything else.
    for client in loaded.clients.values() {
        client.attach_events(&bus);
    }
    let recorder = spawn_recorder(bus.subscribe(), layout.clone())
        .map_err(|err| Refused(format!("cannot write events log: {err}")))?;
    let view_thread = crate::ui::spawn_view(bus.subscribe(), view);
    let runner = Runner::new(bus.clone(), redactor.clone());
    // Steps run in the workspace, so the tools path must not depend on the current directory.
    let tools = recipe_dir.join(recipe.tools.as_deref().unwrap_or("tools"));
    let tools = std::path::absolute(&tools).unwrap_or(tools);
    let ctx = Ctx {
        layout,
        workdir,
        recipe,
        recipe_dir,
        merge: loaded.merge,
        tools: &tools,
        runner: &runner,
        agent: loaded.agent.as_deref(),
        model: loaded.model.as_deref(),
        credentials: &loaded.credentials,
        clients: &loaded.clients,
    };

    let parallel = match mode {
        Mode::DryRun => 1,
        Mode::Pipeline { parallel } => parallel.max(1),
    };
    let mut results = Vec::new();
    let mut pending = 0;
    let waves_total = plan.len();
    for (index, wave) in plan.iter().enumerate() {
        let done = process_all(&ctx, wave, mode, parallel);
        let remaining: usize = plan[index + 1..].iter().map(Vec::len).sum();
        let successful = done.iter().filter(|r| r.successful).count();
        ctx.runner.bus().emit(Event::WaveFinished {
            wave: index + 1,
            total: waves_total,
            successful,
            remaining,
        });
        let next = remaining > 0 && !interrupt_requested() && (waves.proceed)(&done, remaining);
        results.extend(done);
        if remaining > 0 && !next {
            pending = remaining;
            break;
        }
    }

    drop(runner);
    drop(bus);
    let _ = recorder.join();
    // The view ends with the stream; the final table is printed after it has left the terminal.
    if let Some(thread) = view_thread {
        let _ = thread.join();
    }

    meta.finished = rfc3339_now();
    write_meta(layout, &meta).map_err(|err| Refused(err.to_string()))?;
    let mut report = report::build(
        &meta.run_id,
        &meta.recipe,
        &meta.started,
        &meta.finished,
        results,
        &redactor,
    );
    if pending > 0 {
        // Stopped on purpose: 2 when everything done so far succeeded, 1 otherwise.
        report.summary.pending = pending;
        report.exit_code = if report.summary.successful == report.summary.total {
            2
        } else {
            1
        };
    }
    let bytes = serde_json::to_vec_pretty(&report).map_err(|err| Refused(err.to_string()))?;
    write_atomic(&layout.report_path(), &bytes).map_err(|err| Refused(err.to_string()))?;
    Ok(report)
}

/// Everything a per-repo step needs. Shared by all workers.
pub struct Ctx<'a> {
    pub layout: &'a RunLayout,
    pub workdir: &'a Path,
    pub recipe: &'a Recipe,
    /// Where the recipe came from: instruction files and attachments are read from here.
    pub recipe_dir: &'a Path,
    /// Whether this run may merge change requests (`--merge`, already checked at load).
    pub merge: bool,
    /// Trusted copy of the recipe's `tools` directory.
    pub tools: &'a Path,
    pub runner: &'a Runner,
    /// The agent for the recipe's instructions. `None` when there are none.
    pub agent: Option<&'a str>,
    /// The model for `agent`, `None` when the agent's own configuration decides.
    pub model: Option<&'a str>,
    pub credentials: &'a Credentials,
    pub clients: &'a BTreeMap<String, Box<dyn Host>>,
}

/// Processes the repos with `parallel` workers. Results keep the repos' order.
fn process_all(ctx: &Ctx<'_>, repos: &[Repo], mode: Mode, parallel: usize) -> Vec<RepoResult> {
    let next = AtomicUsize::new(0);
    let slots: Mutex<Vec<Option<RepoResult>>> = Mutex::new(vec![None; repos.len()]);
    thread::scope(|scope| {
        for _ in 0..parallel.min(repos.len().max(1)) {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::SeqCst);
                    let Some(repo) = repos.get(index) else { break };
                    let result = if interrupt_requested() {
                        interrupted_before_start(repo)
                    } else {
                        match mode {
                            Mode::DryRun => dry_run_repo(ctx, repo),
                            Mode::Pipeline { .. } => pipeline::process_repo(ctx, repo),
                        }
                    };
                    let _ = report::append_result_locked(&ctx.layout.results_path(), &result);
                    ctx.runner.bus().emit(Event::RepoFinished {
                        repo: result.repo.clone(),
                        result: result.result.clone(),
                        message: ctx.runner.redactor().redact(&result.detail),
                    });
                    slots.lock().unwrap_or_else(|p| p.into_inner())[index] = Some(result);
                }
            });
        }
    });
    slots
        .into_inner()
        .unwrap_or_else(|p| p.into_inner())
        .into_iter()
        .flatten()
        .collect()
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
    for suffix in only {
        if !repos.iter().any(|r| path_matches(r, suffix)) {
            return Err(Refused(format!(
                "--only {suffix}: no repo in {} has this path",
                path.display()
            )));
        }
    }
    Ok(repos
        .into_iter()
        .filter(|r| only.iter().any(|s| path_matches(r, s)))
        .collect())
}

/// One API client per host used by the repos, after a TLS self-test per host. A
/// certificate the OS does not trust stops the run: the internal CA is missing.
/// Other connection failures are reported and the run goes on.
fn host_clients(
    repos: &[Repo],
    max_rate_wait: std::time::Duration,
) -> Result<BTreeMap<String, Box<dyn Host>>, Refused> {
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
        let client =
            crate::hosts::client_for_policy(repo.host_type, &repo.host, None, max_rate_wait)
                .map_err(|err| {
                    Refused(format!("cannot set up the client for {}: {err}", repo.host))
                })?;
        clients.insert(repo.host.clone(), client);
    }
    Ok(clients)
}

/// Folder paths used in failure blocks.
pub fn artifacts(layout: &RunLayout, repo: &Repo) -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "events".to_string(),
            layout.events_path().display().to_string(),
        ),
        ("log".to_string(), layout.log_path().display().to_string()),
        (
            "output".to_string(),
            output_path(layout, &repo.key()).display().to_string(),
        ),
        (
            "repo_dir".to_string(),
            layout.repo_dir(repo).display().to_string(),
        ),
    ])
}

/// A result for a repo that failed with `message`.
pub fn error_result(
    layout: &RunLayout,
    repo: &Repo,
    message: String,
    rule: Option<String>,
    output_tail: Vec<String>,
) -> RepoResult {
    let mut result = RepoResult::new(&repo.key(), "error", &message);
    result.rule = rule.clone();
    result.failure = Some(Failure {
        class: "error".to_string(),
        message,
        rule,
        output_tail,
        artifacts: artifacts(layout, repo),
        next_step: None,
    });
    result
}

/// Whether a git failure came from Ctrl-C; those become `interrupted`.
pub fn git_failure(
    layout: &RunLayout,
    repo: &Repo,
    error: GitError,
    rule: Option<String>,
) -> RepoResult {
    if interrupt_requested() {
        let mut result = RepoResult::new(&repo.key(), "interrupted", &error.message);
        result.failure = Some(Failure {
            class: "interrupted".to_string(),
            message: error.message,
            rule,
            output_tail: error.output_tail,
            artifacts: artifacts(layout, repo),
            next_step: None,
        });
        return result;
    }
    error_result(layout, repo, error.message, rule, error.output_tail)
}

fn interrupted_before_start(repo: &Repo) -> RepoResult {
    let message = "not started: the run was interrupted".to_string();
    let mut result = RepoResult::new(&repo.key(), "interrupted", &message);
    result.failure = Some(Failure {
        class: "interrupted".to_string(),
        message,
        rule: None,
        output_tail: Vec::new(),
        artifacts: BTreeMap::new(),
        next_step: None,
    });
    result
}

/// Credential rule and read-access probe for one repo. Errors come back as a
/// message plus the rule that was involved.
pub fn probe<'a>(
    ctx: &Ctx<'a>,
    repo: &Repo,
) -> Result<(String, &'a Secret), (String, Option<String>)> {
    let rule = ctx
        .credentials
        .find_rule(&repo.host, &repo.path)
        .map_err(|err| (err.to_string(), None))?;
    let pattern = rule.pattern().to_string();
    let client = ctx.clients.get(&repo.host).ok_or_else(|| {
        (
            format!("no API client for {}", repo.host),
            Some(pattern.clone()),
        )
    })?;
    match hosts::retry_read(|| client.probe_read_access(repo, rule.token())) {
        Ok(Access::Ok) => Ok((pattern, rule.token())),
        Ok(denied) => Err((denied.describe().to_string(), Some(pattern))),
        Err(err) => Err((err.to_string(), Some(pattern))),
    }
}

/// Git context for one repo: the token of its rule, the run's scratch dir and the timeout.
pub fn git_context<'a>(
    ctx: &'a Ctx<'_>,
    repo: &'a Repo,
    key: &'a str,
    token: &'a Secret,
    scratch: &'a Path,
) -> GitContext<'a> {
    GitContext {
        runner: ctx.runner,
        label: key,
        scratch,
        auth: Some(Auth {
            username: username_for(repo.host_type.as_str()),
            token,
        }),
        timeout: GIT_TIMEOUT,
    }
}

/// The per-repo step of a dry run: probe, cache, `applies_to`, and the plan.
fn dry_run_repo(ctx: &Ctx<'_>, repo: &Repo) -> RepoResult {
    let key = repo.key();
    let phase = |name: &str| {
        ctx.runner.bus().emit(Event::RepoPhase {
            repo: key.clone(),
            phase: name.to_string(),
        })
    };
    phase("cloning");
    let (pattern, token) = match probe(ctx, repo) {
        Ok(found) => found,
        Err((message, rule)) => return error_result(ctx.layout, repo, message, rule, Vec::new()),
    };
    let scratch = ctx.layout.scratch_dir();
    let git = git_context(ctx, repo, &key, token, &scratch);
    let cache = cache_dir(ctx.workdir, &repo.host, &repo.path);
    let state = match ensure_cache(&git, &repo.clone_url, &cache) {
        Ok(state) => state,
        Err(err) => {
            return crate::pipeline::remote_failure(ctx.layout, &git, repo, err, Some(pattern));
        }
    };
    let state = match crate::pipeline::resolve_base(ctx.recipe, &git, &cache, state) {
        Ok(state) => state,
        Err(message) => return error_result(ctx.layout, repo, message, Some(pattern), Vec::new()),
    };

    phase("checking");
    let facts = RepoFacts {
        host_type: repo.host_type.as_str(),
        path: &repo.path,
        checkout: &cache,
    };
    let unmet = ctx.recipe.unmet_conditions(&facts);
    let mut result = if unmet.is_empty() {
        RepoResult::new(
            &key,
            "dry_run",
            &format!(
                "read access ok; would branch {} from {} at {}",
                ctx.recipe.remote_branch(),
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
mod merge_gate_tests {
    use super::*;

    fn recipe(ci: &str) -> Recipe {
        Recipe::parse(&format!(
            "schema = 1\nname = \"x\"\nbranch = \"b\"\npr_title = \"t\"\n\n[ci]\n{ci}\n"
        ))
        .unwrap()
    }

    #[test]
    fn merge_needs_a_recipe_that_asks_for_it() {
        let asks = recipe("wait = false\nmerge = \"when_green\"");
        let plain = recipe("wait = false\nmerge = \"off\"");
        assert!(check_merge(&asks, false).is_ok());
        assert!(check_merge(&asks, true).is_ok());
        assert!(
            check_merge(&plain, true)
                .unwrap_err()
                .contains("when_green")
        );
    }
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
            host_type: crate::hosts::HostType::Gitlab,
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
