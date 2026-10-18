//! Command line interface. Every stage-1 subcommand exists. Some are still stubs
//! until their milestone lands, and say so on stderr.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::logging;
use crate::recipe::Recipe;
use crate::run::{self, DryRunInput, Refused, Row, RunLayout};
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

impl Command {
    fn name(&self) -> &'static str {
        match self {
            Command::Run(_) => "run",
            Command::Validate { .. } => "validate",
            Command::Schema => "schema",
            Command::Report { .. } => "report",
            Command::Status { .. } => "status",
            Command::Logs { .. } => "logs",
            Command::CheckAgent { .. } => "check-agent",
            Command::Clean { .. } => "clean",
            Command::DebugPanic => "debug-panic",
        }
    }
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
        command => {
            if let Err(code) = start_logging(&workdir.join(TOOL_LOG), debug) {
                return code;
            }
            tracing::info!(
                command = command.name(),
                "subcommand is not implemented yet"
            );
            eprintln!("fleetify: `{}` is not implemented yet", command.name());
            ExitCode::from(1)
        }
    }
}

/// `fleetify run`. Only `--dry-run` is implemented so far.
fn run_command(workdir: &Path, debug: bool, args: RunArgs) -> ExitCode {
    if !args.dry_run {
        if let Err(code) = start_logging(&workdir.join(TOOL_LOG), debug) {
            return code;
        }
        eprintln!("fleetify: `run` without --dry-run is not implemented yet (milestone M7)");
        return ExitCode::from(1);
    }
    if args.format == OutputFormat::Json {
        eprintln!("fleetify: --format json comes with the report (milestone M5)");
        return ExitCode::from(1);
    }

    let recipe = match Recipe::load(&args.recipe_dir) {
        Ok(recipe) => recipe,
        Err(err) => {
            let _ = start_logging(&workdir.join(TOOL_LOG), debug);
            eprintln!("fleetify: {err}");
            return ExitCode::from(3);
        }
    };
    let run_id = run::run_id(&recipe.name, args.run.as_deref(), &run::today_utc());
    let layout = RunLayout::new(workdir, &run_id);
    if let Err(code) = start_logging(&layout.log_path(), debug) {
        return code;
    }
    tracing::info!(run = %run_id, "dry run starts");

    let input = DryRunInput {
        workdir,
        recipe_dir: &args.recipe_dir,
        recipe: &recipe,
        repos_file: &args.repos_file,
        layout: &layout,
        only: &args.only,
        agent: args.agent.as_deref(),
    };
    match run::dry_run(&input) {
        Err(Refused(message)) => {
            tracing::error!(%message, "run refused");
            eprintln!("fleetify: {message}");
            ExitCode::from(3)
        }
        Ok(rows) => {
            print_rows(&run_id, &rows);
            if rows.iter().all(Row::is_success) {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
    }
}

fn print_rows(run_id: &str, rows: &[Row]) {
    println!("run {run_id} (dry run)");
    let width = rows.iter().map(|r| r.repo.len()).max().unwrap_or(4).max(4);
    for row in rows {
        println!(
            "{:<width$}  {:<8}  {:<rule_width$}  {}",
            row.repo,
            row.result,
            row.rule.as_deref().unwrap_or("-"),
            row.detail,
            rule_width = rows
                .iter()
                .map(|r| r.rule.as_deref().unwrap_or("-").len())
                .max()
                .unwrap_or(1),
        );
    }
    let count = |result: &str| rows.iter().filter(|r| r.result == result).count();
    println!(
        "{} dry_run, {} skipped, {} error",
        count("dry_run"),
        count("skipped"),
        count("error")
    );
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
