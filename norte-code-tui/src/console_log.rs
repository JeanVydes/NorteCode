//! Bounded shell output for tool consoles.

use terminalui::components::console::Console;

const MAX_LINES: usize = 500;
const KEEP_LINES: usize = 400;
const VISIBLE_ROWS: usize = 6;

/// Append a line, dropping the oldest output when a command floods the stream.
pub(crate) fn push_line(console: &mut Console, line: impl Into<String>) {
    console.push_line(line);
    if console.lines().len() < MAX_LINES {
        return;
    }

    let skip = console.lines().len() - KEEP_LINES;
    let kept = console.lines()[skip..].to_vec();
    let command = console.command.clone();
    let status = console.status;
    let state = console.state.clone();
    let style = console.style.clone();

    *console = Console::new(command)
        .set_lines(kept)
        .with_max_visible(VISIBLE_ROWS)
        .with_style(style);
    console.status = status;
    console.state.frame = state.frame;
    console.state.elapsed = state.elapsed;
    console.state.offset = state.offset.saturating_sub(skip);
}

#[cfg(test)]
#[path = "console_log_tests.rs"]
mod tests;
