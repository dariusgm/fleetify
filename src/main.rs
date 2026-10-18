use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    // Git runs this same binary as its askpass helper; answer and exit before parsing arguments.
    if let Some(code) = fleetify::git::askpass_helper() {
        return code;
    }
    fleetify::cli::run(fleetify::cli::Cli::parse())
}
