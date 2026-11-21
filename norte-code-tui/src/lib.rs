//! The Norte Code terminal interface, built on terminalui.
//!
//! `norte-code-tui` is the client the `norte` binary delegates to. It talks
//! to the raep server and owns the screen: a retained-mode tree of typed
//! views, central focus management, and a strict raep state machine that
//! never pretends to be attached when it is not.

mod app;
mod client;
mod compact_notice;
mod config;
mod context;
mod diff;
mod job_notice;
mod metrics;
mod palette;
mod process_stats;
mod session_stats;
mod shell;
mod telemetry;
mod telemetry_series;
mod terminal;
mod theme;
mod ui;
mod ui_model;

/// Turns the TUI on and serves until it exits, or reports why it cannot.
///
/// # Errors
/// Returns the error the loop reported to the caller.
pub fn run_tui() -> Result<(), String> {
    app::run().map_err(|error| error.to_string())
}
