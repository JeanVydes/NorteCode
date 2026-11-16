//! The terminal interface, in this process, with the same screen and the
//! same keyboard loop the standalone binary owns.

use norte_code_tui::run_tui;

pub fn run() -> Result<(), String> {
    run_tui()
}
