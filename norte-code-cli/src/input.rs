//! Interactive terminal input with a line editor pinned below the model output.
//!
//! While a turn streams, typed text stays in the input line and never leaks
//! into the answer. Pressing Enter sends the line to the caller, which either
//! starts a turn or queues the line for the next one. The terminal keeps a
//! scroll region above the footer, so streamed output scrolls without touching
//! the pinned rows: the running session totals, the queued-message preview and
//! the input line itself.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The pinned text above the input line.
///
/// The renderer owns the session totals; the input editor owns the terminal.
/// Both share this handle: the renderer replaces the text, the editor redraws
/// it when it changes.
#[derive(Default)]
pub(crate) struct Pinned {
    inner: Mutex<PinnedInner>,
    dirty: AtomicBool,
}

#[derive(Default)]
struct PinnedInner {
    session: String,
    queued: Vec<String>,
}

impl Pinned {
    /// A shared handle for the input editor and the event renderer.
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Replaces the session status line.
    pub(crate) fn set_session(&self, text: String) {
        let mut inner = self.lock();
        if inner.session != text {
            inner.session = text;
            self.dirty.store(true, Ordering::Relaxed);
        }
    }

    /// Replaces the queued-message preview. An empty list clears it.
    pub(crate) fn set_queued(&self, lines: Vec<String>) {
        let mut inner = self.lock();
        if inner.queued != lines {
            inner.queued = lines;
            self.dirty.store(true, Ordering::Relaxed);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, PinnedInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn snapshot(&self) -> (String, Vec<String>) {
        let inner = self.lock();
        (inner.session.clone(), inner.queued.clone())
    }

    fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::Relaxed)
    }
}

#[cfg(unix)]
mod unix {
    use std::io::{self, IsTerminal, Write};
    use std::os::fd::AsRawFd;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use termios::{
        ECHO, ECHOE, ECHOK, ECHONL, ICANON, ICRNL, IXON, TCSANOW, Termios, VMIN, VTIME, tcsetattr,
    };
    use tokio::sync::mpsc;

    use super::Pinned;
    use crate::style::Style;

    /// Rows reserved below the scroll region: session status, queued preview
    /// and the input line.
    const FOOTER_ROWS: u16 = 3;

    /// A running input line editor and its channel of completed lines.
    pub(crate) struct InteractiveInput {
        lines: mpsc::UnboundedReceiver<Option<String>>,
        _terminal: TerminalGuard,
    }

    impl InteractiveInput {
        /// Starts the editor on the current terminal.
        pub(crate) fn start(style: Style, pinned: Arc<Pinned>) -> Result<Self, String> {
            if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
                return Err("interactive input needs a terminal on stdin and stderr".to_owned());
            }
            let fd = io::stdin().as_raw_fd();
            let original = Termios::from_fd(fd)
                .map_err(|error| format!("could not read the terminal settings: {error}"))?;
            let mut edited = original;
            edited.c_lflag &= !(ICANON | ECHO | ECHOE | ECHOK | ECHONL);
            edited.c_iflag &= !(ICRNL | IXON);
            edited.c_cc[VMIN] = 1;
            edited.c_cc[VTIME] = 0;
            tcsetattr(fd, TCSANOW, &edited).map_err(|error| {
                format!("could not switch the terminal to line editing: {error}")
            })?;

            let (cols, rows) = match crossterm::terminal::size() {
                Ok(size) => size,
                Err(error) => {
                    let _ = tcsetattr(fd, TCSANOW, &original);
                    return Err(format!("could not measure the terminal: {error}"));
                }
            };
            if rows < FOOTER_ROWS + 1 {
                let _ = tcsetattr(fd, TCSANOW, &original);
                return Err("the terminal is too small for the input line".to_owned());
            }
            if let Err(error) = enter_terminal(cols, rows) {
                let _ = tcsetattr(fd, TCSANOW, &original);
                return Err(error);
            }

            let (sender, lines) = mpsc::unbounded_channel();
            let stop = Arc::new(AtomicBool::new(false));
            let thread_stop = Arc::clone(&stop);
            let handle = match thread::Builder::new()
                .name("norte-input".to_owned())
                .spawn(move || run_editor(style, cols, rows, &pinned, &sender, &thread_stop))
            {
                Ok(handle) => handle,
                Err(error) => {
                    leave_terminal(rows);
                    let _ = tcsetattr(fd, TCSANOW, &original);
                    return Err(format!("could not start the input reader: {error}"));
                }
            };

            Ok(Self {
                lines,
                _terminal: TerminalGuard {
                    original,
                    stop,
                    handle: Some(handle),
                },
            })
        }

        /// The channel of completed lines. `None` means end of input.
        pub(crate) fn lines(&mut self) -> &mut mpsc::UnboundedReceiver<Option<String>> {
            &mut self.lines
        }
    }

    struct TerminalGuard {
        original: Termios,
        stop: Arc<AtomicBool>,
        handle: Option<JoinHandle<()>>,
    }

    impl Drop for TerminalGuard {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
            leave_terminal(crossterm::terminal::size().map_or(24, |(_, rows)| rows));
            let fd = io::stdin().as_raw_fd();
            let _ = tcsetattr(fd, TCSANOW, &self.original);
        }
    }

    /// The text being edited and the position of the insertion point.
    #[derive(Debug, Default)]
    struct LineBuffer {
        chars: Vec<char>,
        cursor: usize,
    }

    impl LineBuffer {
        fn insert(&mut self, character: char) {
            self.chars.insert(self.cursor, character);
            self.cursor += 1;
        }

        fn insert_text(&mut self, text: &str) {
            for character in text.chars() {
                match character {
                    '\n' | '\r' => self.insert(' '),
                    '\t' => {
                        for _ in 0..4 {
                            self.insert(' ');
                        }
                    }
                    control if control.is_control() => {}
                    other => self.insert(other),
                }
            }
        }

        fn backspace(&mut self) -> bool {
            if self.cursor == 0 {
                return false;
            }
            self.cursor -= 1;
            self.chars.remove(self.cursor);
            true
        }

        fn delete(&mut self) -> bool {
            if self.cursor >= self.chars.len() {
                return false;
            }
            self.chars.remove(self.cursor);
            true
        }

        fn left(&mut self) -> bool {
            if self.cursor == 0 {
                return false;
            }
            self.cursor -= 1;
            true
        }

        fn right(&mut self) -> bool {
            if self.cursor >= self.chars.len() {
                return false;
            }
            self.cursor += 1;
            true
        }

        fn home(&mut self) -> bool {
            let moved = self.cursor != 0;
            self.cursor = 0;
            moved
        }

        fn end(&mut self) -> bool {
            let moved = self.cursor != self.chars.len();
            self.cursor = self.chars.len();
            moved
        }

        fn kill_to_start(&mut self) -> bool {
            if self.cursor == 0 {
                return false;
            }
            self.chars.drain(..self.cursor);
            self.cursor = 0;
            true
        }

        fn kill_to_end(&mut self) -> bool {
            if self.cursor >= self.chars.len() {
                return false;
            }
            self.chars.truncate(self.cursor);
            true
        }

        fn kill_word(&mut self) -> bool {
            if self.cursor == 0 {
                return false;
            }
            let mut start = self.cursor;
            while start > 0 && self.chars[start - 1].is_whitespace() {
                start -= 1;
            }
            while start > 0 && !self.chars[start - 1].is_whitespace() {
                start -= 1;
            }
            self.chars.drain(start..self.cursor);
            self.cursor = start;
            true
        }

        fn clear(&mut self) {
            self.chars.clear();
            self.cursor = 0;
        }

        fn replace(&mut self, text: &str) {
            self.chars = text.chars().collect();
            self.cursor = self.chars.len();
        }

        fn text(&self) -> String {
            self.chars.iter().collect()
        }

        fn is_empty(&self) -> bool {
            self.chars.is_empty()
        }
    }

    fn run_editor(
        style: Style,
        mut cols: u16,
        mut rows: u16,
        pinned: &Pinned,
        sender: &mpsc::UnboundedSender<Option<String>>,
        stop: &AtomicBool,
    ) {
        let mut buffer = LineBuffer::default();
        let mut history: Vec<String> = Vec::new();
        let mut history_pos: Option<usize> = None;
        let mut draft = String::new();

        draw_input(style, cols, rows, &buffer);
        draw_footer(style, cols, rows, pinned);
        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            match event::poll(Duration::from_millis(100)) {
                Ok(true) => {
                    let mut redraw = false;
                    match event::read() {
                        Ok(Event::Key(key))
                            if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                        {
                            match handle_key(
                                key,
                                &mut buffer,
                                &mut history,
                                &mut history_pos,
                                &mut draft,
                                sender,
                            ) {
                                KeyOutcome::Continue => redraw = true,
                                KeyOutcome::Exit => break,
                            }
                        }
                        Ok(Event::Paste(text)) => {
                            buffer.insert_text(&text);
                            redraw = true;
                        }
                        Ok(Event::Resize(width, height)) => {
                            cols = width;
                            rows = height.max(FOOTER_ROWS + 1);
                            reset_region(cols, rows);
                            draw_footer(style, cols, rows, pinned);
                            redraw = true;
                        }
                        _ => {}
                    }
                    if redraw {
                        draw_input(style, cols, rows, &buffer);
                    }
                }
                Ok(false) => {}
                Err(_) => {
                    let _ = sender.send(None);
                    break;
                }
            }
            if pinned.take_dirty() {
                draw_footer(style, cols, rows, pinned);
            }
        }
    }

    enum KeyOutcome {
        Continue,
        Exit,
    }

    #[allow(clippy::too_many_lines)]
    fn handle_key(
        key: KeyEvent,
        buffer: &mut LineBuffer,
        history: &mut Vec<String>,
        history_pos: &mut Option<usize>,
        draft: &mut String,
        sender: &mpsc::UnboundedSender<Option<String>>,
    ) -> KeyOutcome {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Enter => {
                let line = buffer.text();
                if !line.trim().is_empty() {
                    if history.last() != Some(&line) {
                        history.push(line.clone());
                    }
                    history_pos.take();
                    draft.clear();
                }
                if sender.send(Some(line)).is_err() {
                    return KeyOutcome::Exit;
                }
                buffer.clear();
                KeyOutcome::Continue
            }
            KeyCode::Char('d') if control => {
                if buffer.is_empty() {
                    let _ = sender.send(None);
                    return KeyOutcome::Exit;
                }
                KeyOutcome::Continue
            }
            KeyCode::Char('c' | 'l') if control => KeyOutcome::Continue,
            KeyCode::Char('a') if control => {
                buffer.home();
                KeyOutcome::Continue
            }
            KeyCode::Char('e') if control => {
                buffer.end();
                KeyOutcome::Continue
            }
            KeyCode::Char('u') if control => {
                buffer.kill_to_start();
                KeyOutcome::Continue
            }
            KeyCode::Char('k') if control => {
                buffer.kill_to_end();
                KeyOutcome::Continue
            }
            KeyCode::Char('w') if control => {
                buffer.kill_word();
                KeyOutcome::Continue
            }
            KeyCode::Char('b') if control => {
                buffer.left();
                KeyOutcome::Continue
            }
            KeyCode::Char('f') if control => {
                buffer.right();
                KeyOutcome::Continue
            }
            KeyCode::Char('p') if control => {
                history_previous(buffer, history, history_pos, draft);
                KeyOutcome::Continue
            }
            KeyCode::Char('n') if control => {
                history_next(buffer, history, history_pos, draft);
                KeyOutcome::Continue
            }
            KeyCode::Char(character) if !control && !alt => {
                buffer.insert(character);
                *history_pos = None;
                KeyOutcome::Continue
            }
            KeyCode::Backspace => {
                buffer.backspace();
                KeyOutcome::Continue
            }
            KeyCode::Delete => {
                buffer.delete();
                KeyOutcome::Continue
            }
            KeyCode::Left => {
                buffer.left();
                KeyOutcome::Continue
            }
            KeyCode::Right => {
                buffer.right();
                KeyOutcome::Continue
            }
            KeyCode::Home => {
                buffer.home();
                KeyOutcome::Continue
            }
            KeyCode::End => {
                buffer.end();
                KeyOutcome::Continue
            }
            KeyCode::Up => {
                history_previous(buffer, history, history_pos, draft);
                KeyOutcome::Continue
            }
            KeyCode::Down => {
                history_next(buffer, history, history_pos, draft);
                KeyOutcome::Continue
            }
            KeyCode::Tab => {
                for _ in 0..4 {
                    buffer.insert(' ');
                }
                KeyOutcome::Continue
            }
            KeyCode::Esc => {
                buffer.clear();
                *history_pos = None;
                KeyOutcome::Continue
            }
            _ => KeyOutcome::Continue,
        }
    }

    fn history_previous(
        buffer: &mut LineBuffer,
        history: &[String],
        position: &mut Option<usize>,
        draft: &mut String,
    ) {
        if history.is_empty() {
            return;
        }
        match *position {
            None => {
                *draft = buffer.text();
                *position = Some(history.len() - 1);
            }
            Some(index) if index > 0 => *position = Some(index - 1),
            Some(_) => return,
        }
        if let Some(index) = *position {
            buffer.replace(&history[index]);
        }
    }

    fn history_next(
        buffer: &mut LineBuffer,
        history: &[String],
        position: &mut Option<usize>,
        draft: &mut String,
    ) {
        let Some(index) = *position else {
            return;
        };
        if index + 1 < history.len() {
            *position = Some(index + 1);
            buffer.replace(&history[index + 1]);
        } else {
            *position = None;
            let text = std::mem::take(draft);
            buffer.replace(&text);
        }
    }

    /// The bottom row of the scroll region; the footer lives below it.
    fn scroll_bottom(rows: u16) -> u16 {
        rows.saturating_sub(FOOTER_ROWS).max(1)
    }

    fn status_row(rows: u16) -> u16 {
        rows.saturating_sub(2).max(1)
    }

    fn queue_row(rows: u16) -> u16 {
        rows.saturating_sub(1).max(1)
    }

    fn enter_terminal(cols: u16, rows: u16) -> Result<(), String> {
        let mut out = io::stderr().lock();
        write!(out, "\x1b[?25l\x1b[?2004h").map_err(|error| error.to_string())?;
        // The startup and `--debug` blocks are printed before this point. Reset
        // the scroll region, drop to the last row and scroll the footer height
        // so those rows are blank; otherwise the first streamed lines would
        // overwrite the last debug line.
        write!(out, "\x1b[r\x1b[{rows};1H").map_err(|error| error.to_string())?;
        for _ in 0..FOOTER_ROWS {
            writeln!(out).map_err(|error| error.to_string())?;
        }
        set_region(&mut out, cols, rows).map_err(|error| error.to_string())?;
        out.flush().map_err(|error| error.to_string())
    }

    fn leave_terminal(rows: u16) {
        let mut out = io::stderr().lock();
        let _ = write!(out, "\x1b[?2004l\x1b[r");
        // Only the footer rows: the region still holds the final summary.
        for row in scroll_bottom(rows).saturating_add(1)..=rows.max(1) {
            let _ = write!(out, "\x1b[{row};1H\x1b[2K");
        }
        let _ = write!(out, "\x1b[{};1H\x1b[?25h", rows.max(1));
        let _ = out.flush();
    }

    fn reset_region(cols: u16, rows: u16) {
        let mut out = io::stderr().lock();
        let _ = set_region(&mut out, cols, rows);
        let _ = out.flush();
    }

    fn set_region(out: &mut impl Write, cols: u16, rows: u16) -> io::Result<()> {
        let _ = cols;
        let bottom = scroll_bottom(rows);
        write!(out, "\x1b[1;{bottom}r\x1b[{bottom};1H")
    }

    /// Draws the pinned session line and the queued-message preview.
    ///
    /// Both rows are below the scroll region, so streamed output never touches
    /// them. The cursor is saved and restored so the main thread's output
    /// position is preserved.
    fn draw_footer(style: Style, cols: u16, rows: u16, pinned: &Pinned) {
        let (session, queued) = pinned.snapshot();
        let width = usize::from(cols).saturating_sub(2);
        let queue_text = if queued.is_empty() {
            String::new()
        } else {
            let joined = queued.join(" · ");
            format!("{} {}", style.faint("queued"), style.faint(joined))
        };
        let mut out = io::stderr().lock();
        let _ = write!(
            out,
            "\x1b7\x1b[{};1H\x1b[2K{}\x1b[{};1H\x1b[2K{}\x1b8",
            queue_row(rows),
            clip_visible(&queue_text, width),
            status_row(rows),
            clip_visible(&session, width)
        );
        let _ = out.flush();
    }

    /// Clips a styled line to `max` visible columns, keeping ANSI escapes.
    fn clip_visible(text: &str, max: usize) -> String {
        let mut out = String::new();
        let mut visible = 0usize;
        let mut chars = text.chars();
        while let Some(character) = chars.next() {
            if character == '\x1b' {
                out.push(character);
                for next in chars.by_ref() {
                    out.push(next);
                    if next == 'm' {
                        break;
                    }
                }
                continue;
            }
            if visible >= max {
                out.push('…');
                out.push_str("\x1b[0m");
                break;
            }
            out.push(character);
            visible += 1;
        }
        out
    }

    fn draw_input(style: Style, cols: u16, rows: u16, buffer: &LineBuffer) {
        let prompt = format!("{} ", style.cyan("›"));
        let prompt_width = 2usize;
        let available = usize::from(cols).saturating_sub(prompt_width + 1).max(1);
        let offset = buffer.cursor.saturating_sub(available.saturating_sub(1));
        let end = (offset + available).min(buffer.chars.len());
        let mut visible = String::new();
        for index in offset..end {
            let character = buffer.chars[index];
            if index == buffer.cursor {
                visible.push_str("\x1b[7m");
                visible.push(character);
                visible.push_str("\x1b[27m");
            } else {
                visible.push(character);
            }
        }
        if buffer.cursor >= buffer.chars.len() {
            visible.push_str("\x1b[7m \x1b[27m");
        }
        let row = rows.max(1);
        let mut out = io::stderr().lock();
        let _ = write!(out, "\x1b7\x1b[{row};1H\x1b[2K{prompt}{visible}\x1b8");
        let _ = out.flush();
    }

    #[cfg(test)]
    mod tests {
        use super::{LineBuffer, clip_visible};

        #[test]
        fn arrows_move_the_insertion_point_without_reordering_text() {
            let mut buffer = LineBuffer::default();
            buffer.insert_text("abc");
            assert!(buffer.left());
            assert!(buffer.left());
            buffer.insert('X');
            assert_eq!(buffer.text(), "aXbc");
            assert!(buffer.home());
            buffer.insert('0');
            assert_eq!(buffer.text(), "0aXbc");
            assert!(buffer.end());
            buffer.insert('!');
            assert_eq!(buffer.text(), "0aXbc!");
        }

        #[test]
        fn backspace_and_delete_respect_the_cursor() {
            let mut buffer = LineBuffer::default();
            buffer.insert_text("abcd");
            assert!(buffer.left());
            assert!(buffer.backspace());
            assert_eq!(buffer.text(), "abd");
            assert!(buffer.home());
            assert!(buffer.delete());
            assert_eq!(buffer.text(), "bd");
            buffer.insert_text("x");
            assert!(buffer.right());
            assert!(buffer.kill_to_end());
            assert_eq!(buffer.text(), "xb");
            buffer.insert_text("rest");
            assert!(buffer.kill_to_start());
            assert!(buffer.is_empty());
        }

        #[test]
        fn control_word_editing_does_not_eat_the_prompt() {
            let mut buffer = LineBuffer::default();
            buffer.insert_text("hello brave world");
            assert!(buffer.kill_word());
            assert_eq!(buffer.text(), "hello brave ");
            assert!(buffer.kill_to_start());
            assert_eq!(buffer.text(), "");
        }

        #[test]
        fn pasted_newlines_stay_inside_one_line() {
            let mut buffer = LineBuffer::default();
            buffer.insert_text("one\r\ntwo\tthree");
            assert_eq!(buffer.text(), "one  two    three");
        }

        #[test]
        fn footer_clipping_counts_visible_columns_and_keeps_styles() {
            // Six visible characters plus two escape sequences.
            let text = "\x1b[2mabcdef\x1b[0m";
            assert_eq!(clip_visible(text, 10), text);
            let clipped = clip_visible(text, 3);
            assert_eq!(clipped, "\x1b[2mabc…\x1b[0m");
            assert_eq!(clip_visible("plain", 5), "plain");
        }
    }
}

#[cfg(unix)]
pub(crate) use unix::InteractiveInput;

#[cfg(not(unix))]
mod fallback {
    use std::io;
    use std::sync::Arc;

    use tokio::sync::mpsc;

    use crate::style::Style;

    /// A plain line reader for platforms without the Unix terminal editor.
    pub(crate) struct InteractiveInput {
        lines: mpsc::UnboundedReceiver<Option<String>>,
    }

    impl InteractiveInput {
        /// Starts a blocking line reader on stdin.
        pub(crate) fn start(_style: Style, _pinned: Arc<super::Pinned>) -> Result<Self, String> {
            let (sender, lines) = mpsc::unbounded_channel();
            std::thread::spawn(move || {
                loop {
                    let mut line = String::new();
                    match io::stdin().read_line(&mut line) {
                        Ok(0) | Err(_) => {
                            let _ = sender.send(None);
                            break;
                        }
                        Ok(_) => {
                            if sender.send(Some(line.trim_end().to_owned())).is_err() {
                                break;
                            }
                        }
                    }
                }
            });
            Ok(Self { lines })
        }

        /// The channel of completed lines.
        pub(crate) fn lines(&mut self) -> &mut mpsc::UnboundedReceiver<Option<String>> {
            &mut self.lines
        }
    }
}

#[cfg(not(unix))]
pub(crate) use fallback::InteractiveInput;
