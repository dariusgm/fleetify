use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    fleetify::cli::run(fleetify::cli::Cli::parse())
}
