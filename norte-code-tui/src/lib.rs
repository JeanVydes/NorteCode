//! The terminal user interface of Norte Code.
//!
//! This library is the front-end the `norte` binary delegates to; it owns the
//! screen and the conversation loop. What it does not own is the host: it
//! talks to one through the client, and expects that host to be running —
//! opening one implies keeping one.

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
