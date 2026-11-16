//! Terminal interface for Norte Code.

#![forbid(unsafe_code)]

use norte_code_tui::run_tui;

fn main() -> Result<(), String> {
    run_tui()
}
