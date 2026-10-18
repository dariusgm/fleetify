//! Command line interface. Every stage-1 subcommand exists; most are stubs
//! until their milestone lands and then say so on stderr.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};

use crate::logging;

/// Name of the tool log inside the working directory. Run folders (M4) get
/// their own `fleetify.log`; until then the tool logs here.
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

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run a recipe over the repositories listed in a repos file
    Run {
        recipe_dir: PathBuf,
        repos_file: PathBuf,
        /// Show what would happen without creating workspaces or pushing
        #[arg(long)]
        dry_run: bool,
        /// Only repos whose path ends with one of these suffixes
        #[arg(long, value_delimiter = ',')]
        only: Vec<String>,
        /// Number of repos processed concurrently
        #[arg(long, default_value_t = 1)]
        parallel: usize,
        /// Run id (default: <recipe>-<YYYY-MM-DD>)
        #[arg(long)]
        run: Option<String>,
        /// Agent to use when the recipe names none
        #[arg(long)]
        agent: Option<String>,
        /// Wave sizes, e.g. "1,all"
        #[arg(long, default_value = "1,all")]
        waves: String,
        /// Repos used as wave 1 instead of the first one
        #[arg(long, value_delimiter = ',')]
        canary_repos: Vec<String>,
        /// Continue to the next wave automatically when the current one is green
        #[arg(long)]
        continue_if_green: bool,
        /// Merge after CI is green (second key; needs `ci.merge = "when_green"`)
        #[arg(long)]
        merge: bool,
        /// Allow agents that did not pass `check-agent`
        #[arg(long)]
        allow_unchecked_agent: bool,
        /// Plain output even on a terminal
        #[arg(long, conflicts_with = "ui")]
        plain: bool,
        /// Live terminal view (ignored when stdout is not a terminal)
        #[arg(long)]
        ui: bool,
        /// Lines of failing output shown by the live view
        #[arg(long, default_value_t = 6)]
        tail: usize,
        /// Final output format on stdout
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
        /// Also write the report to this path
        #[arg(long, value_name = "PATH")]
        report: Option<PathBuf>,
    },
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
            Command::Run { .. } => "run",
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

/// Entry point after argument parsing.
pub fn run(cli: Cli) -> ExitCode {
    let Some(workdir) = resolve_workdir(cli.workdir.clone()) else {
        eprintln!("fleetify: cannot determine the working directory; pass --workdir");
        return ExitCode::from(1);
    };
    let log_path = workdir.join(TOOL_LOG);
    if let Err(err) = logging::init(&log_path, cli.debug) {
        eprintln!("fleetify: cannot open {}: {err}", log_path.display());
        return ExitCode::from(1);
    }
    logging::install_panic_hook(log_path);

    match cli.command {
        Command::DebugPanic => panic!("deliberate panic requested with the debug-panic subcommand"),
        Command::Schema => {
            println!("{}", crate::recipe::json_schema());
            ExitCode::SUCCESS
        }
        Command::Validate { recipe_dir, agent } => validate_command(&recipe_dir, agent.as_deref()),
        command => {
            tracing::info!(
                command = command.name(),
                "subcommand is not implemented yet"
            );
            eprintln!("fleetify: `{}` is not implemented yet", command.name());
            ExitCode::from(1)
        }
    }
}

/// `fleetify validate`: prints the rendered commands on success. Exit code 3 when the
/// recipe is refused, with every issue listed on stderr.
fn validate_command(dir: &std::path::Path, agent: Option<&str>) -> ExitCode {
    let recipe = match crate::recipe::Recipe::load(dir) {
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
