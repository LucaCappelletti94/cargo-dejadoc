//! The action smoke harness, ported from the Python harness.
//!
//! It parses `action.yml`, executes its real `run` steps in a fake runner
//! environment with fixture `cargo` and `gh` binaries on the `PATH`, one
//! fixture report per scenario, and a fake `github` context, and records the
//! `uses` steps and every logged call for declarative checks.
mod expr;
pub mod fix;
pub mod runner;
pub mod world;
