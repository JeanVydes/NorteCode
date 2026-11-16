#![allow(clippy::all, clippy::pedantic)]
//! Pretty printing helpers: colors, boxes and a tiny spinner.

pub fn accent(text: &str) -> String {
    format!("\x1b[38;5;213m{text}\x1b[0m")
}
pub fn bold(text: &str) -> String {
    format!("\x1b[1m{text}\x1b[0m")
}
pub fn muted(text: &str) -> String {
    format!("\x1b[38;5;245m{text}\x1b[0m")
}
pub fn red(text: &str) -> String {
    format!("\x1b[38;5;203m{text}\x1b[0m")
}
pub fn ok(text: &str) -> String {
    format!("\x1b[38;5;121m{text}\x1b[0m")
}
pub fn heading(text: &str) -> String {
    format!("\x1b[1;38;5;213m{text}\x1b[0m")
}
pub fn dim(text: &str) -> String {
    format!("\x1b[38;5;240m{text}\x1b[0m")
}
pub fn print_kv(key: &str, value: &str) {
    println!("  {}  {}", muted(key), value);
}
pub fn print_heading(text: &str) {
    println!();
    println!("  {}", heading(text));
    println!("  {}", dim(&"─".repeat(54)));
}
pub fn spinner_frames() -> &'static [char] {
    &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏']
}

pub fn warn(text: &str) -> String {
    format!("\x1b[38;5;179m{text}\x1b[0m")
}
