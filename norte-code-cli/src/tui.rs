//! The terminal interface, in this process, with the same screen and the
//! same keyboard loop the standalone binary owns.

use norte_code_tui::{TuiError, run_tui};

pub fn run() -> Result<(), TuiError> {
    run_tui()
}
