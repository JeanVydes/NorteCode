//! The `norte-code-tui` binary.
//!
//! The terminal interface delegates here; everything lives in the library.

#![forbid(unsafe_code)]

fn main() -> Result<(), String> {
    norte_code_tui::run_tui()
}
