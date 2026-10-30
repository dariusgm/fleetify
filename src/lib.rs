//! `fleetify`: run one task across many Git repositories, with an optional
//! coding agent, a verifier and a CI gate. See `README.md` and `docs/design/` for the design.

pub mod agents;
pub mod ci;
pub mod ci_config;
pub mod cli;
pub mod clock;
pub mod container;
pub mod credentials;
pub mod egress;
pub mod events;
pub mod git;
pub mod grants;
pub mod guards;
pub mod hosts;
pub mod isolation;
pub mod live;
pub mod logging;
pub mod pipeline;
pub mod recipe;
pub mod report;
pub mod repos;
pub mod run;
pub mod runner;
pub mod state;
pub mod ui;
pub mod workspace;
