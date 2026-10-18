use clap::CommandFactory;
use fleetify::cli::Cli;

#[test]
fn all_stage_one_subcommands_exist() {
    let cmd = Cli::command();
    let names: Vec<&str> = cmd.get_subcommands().map(|c| c.get_name()).collect();
    for expected in [
        "run",
        "validate",
        "schema",
        "report",
        "status",
        "logs",
        "check-agent",
        "clean",
    ] {
        assert!(names.contains(&expected), "missing subcommand {expected}");
    }
}

#[test]
fn cli_definition_is_valid() {
    Cli::command().debug_assert();
}
