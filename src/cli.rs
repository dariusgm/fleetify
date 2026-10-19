//! Command line interface. Every stage-1 subcommand exists. Some are still stubs
//! until their milestone lands, and say so on stderr.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::agents;
use crate::clock::today_utc;
use crate::logging;
use crate::recipe::Recipe;
use crate::report::{self, Report};
use crate::run::{self, Refused, RunInput, RunLayout};
use crate::state;
use crate::workspace::remove_workspaces;

/// Tool log for commands that have no run folder.
const TOOL_LOG: &str = "fleetify.log";

#[derive(Debug, Parser)]
#[command(
    name = "fleetify",
    version,
    about = "Run one task across many Git repositories."
)]
pub struct Cli {
    /// Working directory for clones, runs and logs [default: ~/.fleetify]
    #[arg(long, global = true, value_name = "DIR")]
    pub workdir: Option<PathBuf>,

    /// Log at debug level (RUST_LOG takes precedence)
    #[arg(long, global = true)]
    pub debug: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Table,
    Json,
}

#[derive(Debug, Args)]
pub struct RunArgs {
    pub recipe_dir: PathBuf,
    pub repos_file: PathBuf,
    /// Show what would happen without creating workspaces or pushing
    #[arg(long)]
    pub dry_run: bool,
    /// Only repos whose path ends with one of these suffixes
    #[arg(long, value_delimiter = ',')]
    pub only: Vec<String>,
    /// Number of repos processed concurrently
    #[arg(long, default_value_t = 1)]
    pub parallel: usize,
    /// Run id (default: <recipe>-<YYYY-MM-DD>)
    #[arg(long)]
    pub run: Option<String>,
    /// Agent to use when the recipe names none
    #[arg(long)]
    pub agent: Option<String>,
    /// Wave sizes, e.g. "1,all"
    #[arg(long, default_value = "1,all")]
    pub waves: String,
    /// Repos used as wave 1 instead of the first one
    #[arg(long, value_delimiter = ',')]
    pub canary_repos: Vec<String>,
    /// Continue to the next wave automatically when the current one is green
    #[arg(long)]
    pub continue_if_green: bool,
    /// Merge after CI is green (second key; needs `ci.merge = "when_green"`)
    #[arg(long)]
    pub merge: bool,
    /// Allow agents that did not pass `check-agent`
    #[arg(long)]
    pub allow_unchecked_agent: bool,
    /// Plain output even on a terminal
    #[arg(long, conflicts_with = "ui")]
    pub plain: bool,
    /// Live terminal view (ignored when stdout is not a terminal)
    #[arg(long)]
    pub ui: bool,
    /// Lines of failing output shown by the live view
    #[arg(long, default_value_t = 6)]
    pub tail: usize,
    /// Final output format on stdout
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,
    /// Also write the report to this path
    #[arg(long, value_name = "PATH")]
    pub report: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run a recipe over the repositories listed in a repos file
    Run(RunArgs),
    /// Check a recipe: schema, referenced files and rendered commands per OS
    Validate {
        recipe_dir: PathBuf,
        /// Agent to assume when the recipe names none
        #[arg(long)]
        agent: Option<String>,
    },
    /// Print the JSON Schema of recipe.toml for this binary
    Schema,
    /// Render the report of a run
    Report {
        #[arg(long)]
        run: Option<String>,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
        /// Only repos that were not successful
        #[arg(long)]
        failed: bool,
    },
    /// Show per-repo state of a run (usable while it is active)
    Status {
        #[arg(long)]
        run: Option<String>,
        #[arg(long)]
        watch: bool,
    },
    /// Show the agent log of one repo
    Logs {
        repo: String,
        #[arg(long)]
        run: Option<String>,
        #[arg(short = 'f', long)]
        follow: bool,
        #[arg(short = 'n', long, default_value_t = 50)]
        lines: usize,
    },
    /// Canary test: is the agent confined to its workspace?
    CheckAgent { name: String },
    /// Delete the workspaces of a run (clones are kept)
    Clean {
        /// Run to clean [default: the most recently modified run]
        #[arg(long)]
        run: Option<String>,
    },
    /// Test hook: panics on purpose to exercise the crash path (exit code 4)
    #[command(hide = true)]
    DebugPanic,
}

/// Resolves the working directory: `--workdir`, else `~/.fleetify`.
pub fn resolve_workdir(explicit: Option<PathBuf>) -> Option<PathBuf> {
    explicit.or_else(|| {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
        Some(PathBuf::from(home).join(".fleetify"))
    })
}

/// Starts logging to `path` and installs the crash hook for it.
fn start_logging(path: &Path, debug: bool) -> Result<(), ExitCode> {
    if let Err(err) = logging::init(path, debug) {
        eprintln!("fleetify: cannot open {}: {err}", path.display());
        return Err(ExitCode::from(1));
    }
    logging::install_panic_hook(path.to_path_buf());
    Ok(())
}

/// Entry point after argument parsing.
pub fn run(cli: Cli) -> ExitCode {
    let Some(workdir) = resolve_workdir(cli.workdir.clone()) else {
        eprintln!("fleetify: cannot determine the working directory; pass --workdir");
        return ExitCode::from(1);
    };
    let debug = cli.debug;

    match cli.command {
        Command::Run(args) => run_command(&workdir, debug, args),
        Command::DebugPanic => {
            if let Err(code) = start_logging(&workdir.join(TOOL_LOG), debug) {
                return code;
            }
            panic!("deliberate panic requested with the debug-panic subcommand")
        }
        Command::Schema => {
            println!("{}", crate::recipe::json_schema());
            ExitCode::SUCCESS
        }
        Command::Validate { recipe_dir, agent } => validate_command(&recipe_dir, agent.as_deref()),
        Command::Clean { run } => clean_command(&workdir, debug, run),
        Command::Report {
            run,
            format,
            failed,
        } => report_command(&workdir, debug, run, format, failed),
        Command::Status { run, watch } => status_command(&workdir, debug, run, watch),
        Command::Logs {
            repo,
            run,
            follow,
            lines,
        } => logs_command(&workdir, debug, &repo, run, follow, lines),
        Command::CheckAgent { name } => check_agent_command(&workdir, debug, &name),
    }
}

/// `fleetify run`. A dry run plans; a real run executes the no-agent pipeline.
fn run_command(workdir: &Path, debug: bool, args: RunArgs) -> ExitCode {
    let refusal = if args.merge {
        Some("--merge comes with milestone M11")
    } else if args.continue_if_green || args.waves != "all" && args.waves != "1,all" {
        Some("waves and --continue-if-green come with milestone M8; omit them or pass --waves all")
    } else {
        None
    };
    let recipe = match Recipe::load(&args.recipe_dir) {
        Ok(recipe) => recipe,
        Err(err) => {
            let _ = start_logging(&workdir.join(TOOL_LOG), debug);
            eprintln!("fleetify: {err}");
            return ExitCode::from(3);
        }
    };
    let run_id = run::run_id(&recipe.name, args.run.as_deref(), &today_utc());
    let layout = RunLayout::new(workdir, &run_id);
    if let Err(code) = start_logging(&layout.log_path(), debug) {
        return code;
    }
    if let Some(message) = refusal {
        eprintln!("fleetify: {message}");
        return ExitCode::from(3);
    }
    // The first Ctrl-C stops running commands and marks the rest of the run interrupted.
    let _ = ctrlc::set_handler(crate::runner::request_interrupt);
    let mode = if args.dry_run {
        run::Mode::DryRun
    } else {
        run::Mode::Pipeline {
            parallel: args.parallel,
        }
    };
    tracing::info!(run = %run_id, dry_run = args.dry_run, "run starts");

    let input = RunInput {
        workdir,
        recipe_dir: &args.recipe_dir,
        recipe: &recipe,
        repos_file: &args.repos_file,
        layout: &layout,
        only: &args.only,
        agent: args.agent.as_deref(),
        allow_unchecked_agent: args.allow_unchecked_agent,
    };
    match run::execute(&input, mode) {
        Err(Refused(message)) => {
            tracing::error!(%message, "run refused");
            eprintln!("fleetify: {message}");
            ExitCode::from(3)
        }
        Ok(report) => {
            if let Some(path) = &args.report
                && let Err(err) = write_json(path, &report)
            {
                eprintln!("fleetify: cannot write {}: {err}", path.display());
                return ExitCode::from(1);
            }
            match args.format {
                OutputFormat::Json => print_json(&report),
                OutputFormat::Table => print!("{}", report::render_table(&report, false)),
            }
            ExitCode::from(u8::try_from(report.exit_code).unwrap_or(1))
        }
    }
}

fn write_json(path: &Path, report: &Report) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(report).map_err(std::io::Error::other)?;
    std::fs::write(path, bytes)
}

fn print_json(report: &Report) {
    match serde_json::to_string_pretty(report) {
        Ok(text) => println!("{text}"),
        Err(err) => eprintln!("fleetify: cannot render report: {err}"),
    }
}

/// Resolves `--run` or the most recently modified run.
fn resolve_run(workdir: &Path, requested: Option<String>) -> Result<RunLayout, String> {
    let run_id = match requested {
        Some(id) => id,
        None => run::latest_run(workdir)
            .map_err(|err| format!("cannot list runs: {err}"))?
            .ok_or_else(|| format!("no runs in {}", workdir.display()))?,
    };
    let layout = RunLayout::new(workdir, &run_id);
    if !layout.root.is_dir() {
        return Err(format!("run {run_id} does not exist"));
    }
    Ok(layout)
}

/// `fleetify report`: the report of a run as a table or JSON.
fn report_command(
    workdir: &Path,
    debug: bool,
    requested: Option<String>,
    format: OutputFormat,
    failed: bool,
) -> ExitCode {
    if let Err(code) = start_logging(&workdir.join(TOOL_LOG), debug) {
        return code;
    }
    let layout = match resolve_run(workdir, requested) {
        Ok(layout) => layout,
        Err(message) => {
            eprintln!("fleetify: {message}");
            return ExitCode::from(1);
        }
    };
    let mut report = match run::load_report(&layout) {
        Ok(report) => report,
        Err(message) => {
            eprintln!("fleetify: {message}");
            return ExitCode::from(1);
        }
    };
    if failed {
        report.repos.retain(|r| !r.successful);
    }
    match format {
        OutputFormat::Json => print_json(&report),
        OutputFormat::Table => print!("{}", report::render_table(&report, failed)),
    }
    ExitCode::SUCCESS
}

/// `fleetify status`: one line per repo from the state files. With `--watch`,
/// the table is printed again every few seconds until the run has finished.
fn status_command(workdir: &Path, debug: bool, requested: Option<String>, watch: bool) -> ExitCode {
    if let Err(code) = start_logging(&workdir.join(TOOL_LOG), debug) {
        return code;
    }
    let layout = match resolve_run(workdir, requested) {
        Ok(layout) => layout,
        Err(message) => {
            eprintln!("fleetify: {message}");
            return ExitCode::from(1);
        }
    };
    loop {
        match state::read_states(&layout.root) {
            Ok(states) => print!("{}", render_status(&layout.run_id, &states)),
            Err(err) => {
                eprintln!("fleetify: cannot read run state: {err}");
                return ExitCode::from(1);
            }
        }
        let finished = std::fs::read_to_string(layout.meta_path())
            .ok()
            .and_then(|text| serde_json::from_str::<run::RunMeta>(&text).ok())
            .is_some_and(|meta| !meta.finished.is_empty());
        if !watch || finished {
            return ExitCode::SUCCESS;
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
        println!();
    }
}

fn render_status(run_id: &str, states: &[state::RepoState]) -> String {
    let now = crate::clock::unix_now();
    let mut out = format!("run {run_id}\n");
    let width = states
        .iter()
        .map(|s| s.repo.len())
        .max()
        .unwrap_or(4)
        .max(4);
    for state in states {
        let elapsed = now.saturating_sub(state.started_unix);
        let idle = now.saturating_sub(state.last_output_unix.unwrap_or(state.updated_unix));
        let last = state.last_lines.last().map(String::as_str).unwrap_or("");
        out.push_str(&format!(
            "{:<width$}  {:<12}  elapsed {:>8}  idle {:>8}  {}\n",
            state.repo,
            state.result.as_deref().unwrap_or(&state.phase),
            format_secs(elapsed),
            format_secs(idle),
            last
        ));
    }
    out
}

fn format_secs(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m{:02}s", secs / 60, secs % 60),
        _ => format!("{}h{:02}m", secs / 3600, secs % 3600 / 60),
    }
}

/// `fleetify logs <repo>`: the output log of one repo. `-f` keeps printing new lines.
fn logs_command(
    workdir: &Path,
    debug: bool,
    repo: &str,
    requested: Option<String>,
    follow: bool,
    lines: usize,
) -> ExitCode {
    if let Err(code) = start_logging(&workdir.join(TOOL_LOG), debug) {
        return code;
    }
    let layout = match resolve_run(workdir, requested) {
        Ok(layout) => layout,
        Err(message) => {
            eprintln!("fleetify: {message}");
            return ExitCode::from(1);
        }
    };
    let states = match state::read_states(&layout.root) {
        Ok(states) => states,
        Err(err) => {
            eprintln!("fleetify: cannot read run state: {err}");
            return ExitCode::from(1);
        }
    };
    let matches: Vec<&state::RepoState> = states
        .iter()
        .filter(|s| s.repo == repo || s.repo.ends_with(&format!("/{repo}")))
        .collect();
    let key = match matches.as_slice() {
        [one] => one.repo.clone(),
        [] => {
            eprintln!("fleetify: no repo {repo} in run {}", layout.run_id);
            return ExitCode::from(1);
        }
        _ => {
            eprintln!("fleetify: {repo} matches several repos; use the full host/path");
            return ExitCode::from(1);
        }
    };
    let path = state::output_path(&layout, &key);
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let all: Vec<&str> = text.lines().collect();
    for line in &all[all.len().saturating_sub(lines)..] {
        println!("{line}");
    }
    if !follow {
        return ExitCode::SUCCESS;
    }
    let mut offset = text.len();
    loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if text.len() > offset {
            print!("{}", &text[offset..]);
            offset = text.len();
        }
    }
}

/// `fleetify check-agent <name>`: the canary. Exit code 1 when the agent fails it.
fn check_agent_command(workdir: &Path, debug: bool, name: &str) -> ExitCode {
    if let Err(code) = start_logging(&workdir.join(TOOL_LOG), debug) {
        return code;
    }
    match agents::check(workdir, name) {
        Ok(()) => {
            println!("agent {name} passed check-agent");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("fleetify: agent {name} failed check-agent: {message}");
            ExitCode::from(1)
        }
    }
}

/// `fleetify validate`: prints the rendered commands on success. Exit code 3 when the
/// recipe is refused, with every issue listed on stderr.
fn validate_command(dir: &Path, agent: Option<&str>) -> ExitCode {
    let recipe = match Recipe::load(dir) {
        Ok(recipe) => recipe,
        Err(err) => {
            eprintln!("fleetify: {err}");
            return ExitCode::from(3);
        }
    };
    let issues = recipe.validate(dir, agent);
    if !issues.is_empty() {
        for issue in &issues {
            eprintln!("error: {issue}");
        }
        eprintln!(
            "fleetify: recipe {} refused: {} issue(s)",
            recipe.name,
            issues.len()
        );
        return ExitCode::from(3);
    }
    let rendered = match recipe.rendered_commands(dir) {
        Ok(rendered) => rendered,
        Err(err) => {
            eprintln!("fleetify: {err}");
            return ExitCode::from(3);
        }
    };
    println!("recipe {} is valid", recipe.name);
    println!("branch: {}", recipe.branch);
    for command in rendered {
        println!(
            "{} [{}]: {}",
            command.step,
            command.os,
            display_argv(&command.argv)
        );
    }
    ExitCode::SUCCESS
}

/// `fleetify clean`: removes the workspaces of one run. Clones stay.
fn clean_command(workdir: &Path, debug: bool, requested: Option<String>) -> ExitCode {
    if let Err(code) = start_logging(&workdir.join(TOOL_LOG), debug) {
        return code;
    }
    let run_id = match requested {
        Some(id) => id,
        None => match run::latest_run(workdir) {
            Ok(Some(id)) => id,
            Ok(None) => {
                eprintln!("fleetify: no runs in {}", workdir.display());
                return ExitCode::from(1);
            }
            Err(err) => {
                eprintln!("fleetify: cannot list runs: {err}");
                return ExitCode::from(1);
            }
        },
    };
    let layout = RunLayout::new(workdir, &run_id);
    if !layout.root.is_dir() {
        eprintln!("fleetify: run {run_id} does not exist");
        return ExitCode::from(1);
    }
    match remove_workspaces(&layout.root) {
        Ok(count) => {
            println!("run {run_id}: removed {count} workspace(s); clones and logs kept");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("fleetify: cannot clean run {run_id}: {err}");
            ExitCode::from(1)
        }
    }
}

/// Shows an argument list for people to read. Not for execution: arguments with
/// spaces or shell characters are single-quoted.
fn display_argv(argv: &[String]) -> String {
    argv.iter()
        .map(|arg| {
            let plain = !arg.is_empty()
                && arg
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_./:=@+{}<>".contains(c));
            if plain {
                arg.clone()
            } else {
                format!("'{}'", arg.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
