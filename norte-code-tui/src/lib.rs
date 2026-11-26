//! The Norte Code terminal interface, built on terminalui.
//!
//! `norte-code-tui` is the client the `norte` binary delegates to. It talks
//! to the raep server and owns the screen: a retained-mode tree of typed
//! views, central focus management, and a strict raep state machine that
//! never pretends to be attached when it is not.

mod answer;
mod app;
mod attachment;
mod chip;
mod client;
mod client_error;
mod clipboard;
mod compact_notice;
mod config;
mod console_log;
mod context;
mod diff;
mod frame_limiter;
mod job_notice;
mod leaderboard;
pub mod lifecycle;
mod metrics;
mod palette;
mod plugin_widgets;
mod process_stats;
mod review_feedback;
mod session_stats;
mod shared_telemetry;
mod shell;
mod telemetry;
mod telemetry_series;
mod terminal;
mod theme;
mod tui_error;
mod ui;
mod ui_model;
mod usage_calendar;

pub use tui_error::TuiError;

#[cfg(test)]
mod test_support;

/// Turns the TUI on and serves until it exits, or reports why it cannot.
///
/// # Errors
/// Returns the error the loop reported to the caller.
pub fn run_tui() -> Result<(), TuiError> {
    lifecycle::run_with_lifecycle(
        |_| Ok(()),
        || {
            let path = norte_code_protocol::config::user_cli_path();
            let config = norte_code_protocol::config::CliConfig::load(path.as_deref(), None)
                .map_err(std::io::Error::other)?
                .config;
            if config.service_type == norte_code_protocol::config::ServiceType::Remote {
                return Ok(());
            }
            let status = std::process::Command::new("norte")
                .args(["service", "down"])
                .status()?;
            if status.success() {
                Ok(())
            } else {
                Err(std::io::Error::other("could not stop local service"))
            }
        },
    )
    .map_err(Into::into)
}
