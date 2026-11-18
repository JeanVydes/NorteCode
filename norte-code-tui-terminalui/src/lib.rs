//! The Norte Code terminal interface, built on terminalui.
//!
//! `norte-code-tui-terminalui` is the client the `norte` binary delegates
//! to. It talks to the raep server through the same client the previous
//! TUI used; what changed is the presentation: a retained-mode tree of
//! typed views, central focus management, and a strict raep state machine
//! that never pretends to be attached when it is not.

mod app;
mod client;
mod config;
mod terminal;
mod theme;
mod ui;
mod ui_model;

/// Turns the TUI on and serves until it exits, or reports why it cannot.
///
/// # Errors
/// Returns the error the loop reported to the caller.
///
/// # Notes
/// The previous implementation launched the raep service itself; that
/// coupling moves here with the same behavior. The service is not managed
/// by this process; a `service_type` of `local` only reports intent.
pub fn run_tui() -> Result<(), String> {
    app::run().map_err(|error| error.to_string())
}
