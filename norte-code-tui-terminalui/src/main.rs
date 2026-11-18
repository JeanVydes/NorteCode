//! The `norte-code-tui-terminalui` binary.
//!
//! The terminal interface delegates here; everything lives in the library.

#![forbid(unsafe_code)]

fn main() -> Result<(), String> {
    norte_code_tui_terminalui::run_tui()
}
