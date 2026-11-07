use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    // Git runs this same binary as its askpass helper; answer and exit before parsing arguments.
    if let Some(code) = fleetify::git::askpass_helper() {
        return code;
    }
    // The `.env` file in the working directory, before anything reads a variable.
    if let Ok(cwd) = std::env::current_dir()
        && let Err(message) = fleetify::envfile::load_from(&cwd)
    {
        eprintln!("fleetify: {message}");
        return ExitCode::FAILURE;
    }
    fleetify::cli::run(fleetify::cli::Cli::parse())
}
