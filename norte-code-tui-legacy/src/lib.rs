//! The legacy Norte Code terminal interface.

#![forbid(unsafe_code)]

mod app;
mod chips;
mod client;
mod components;
mod config;
mod keys;
mod provider_keys;
mod signals;
mod terminal;
mod text_builder;
mod tools;
mod ui_model;

/// Turns the TUI off gracefully, or reports why it could not start.
///
/// # Errors
///
/// Returns the error the loop reported to the caller.
pub fn run_tui() -> Result<(), String> {
    app::run().map_err(|error| error.to_string())
}
