//! `fleetify`: run one task across many Git repositories, with an optional
//! coding agent, a verifier and a CI gate. See `PLAN.md` for the design.

pub mod agents;
pub mod ci;
pub mod cli;
pub mod config;
pub mod events;
pub mod hosts;
pub mod logging;
pub mod recipe;
pub mod report;
pub mod repos;
pub mod runner;
pub mod ui;
