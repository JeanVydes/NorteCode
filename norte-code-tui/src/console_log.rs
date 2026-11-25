//! Bounded shell output for tool consoles.

use norte_code_client::config::RetentionConfig;
use terminalui::components::console::Console;

/// Append a line, dropping the oldest output when a command floods the stream.
pub(crate) fn push_line(
    console: &mut Console,
    line: impl Into<String>,
    stream: norte_code_wire::OutputStream,
    retention: RetentionConfig,
) {
    match stream {
        norte_code_wire::OutputStream::Stderr => console.push_error_line(line),
        norte_code_wire::OutputStream::Stdout | norte_code_wire::OutputStream::Other => {
            console.push_line(line);
        }
    }
    trim_output(console, retention);
}

/// Append a streaming chunk without splitting lines at event boundaries.
pub(crate) fn push_chunk(
    console: &mut Console,
    chunk: &str,
    stream: norte_code_wire::OutputStream,
    retention: RetentionConfig,
) {
    console.push_chunk(chunk, stream == norte_code_wire::OutputStream::Stderr);
    trim_output(console, retention);
}

fn trim_output(console: &mut Console, retention: RetentionConfig) {
    if console.lines().len() >= retention.console_max_lines {
        console.retain_last_lines(retention.console_keep_lines.max(1));
    }
}

#[cfg(test)]
#[path = "console_log_tests.rs"]
mod tests;
