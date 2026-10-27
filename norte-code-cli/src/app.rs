//! The interface: the state it shows, and the frame that shows it.
//!
//! One loop, one terminal, one frame at a time. The loop waits on three things
//! — a key, an event from the service, and the tick that keeps a spinner moving
//! — and draws when one of them changed something. In between, it draws nothing:
//! a client sitting at a prompt costs no CPU.
//!
//! Everything visible is [`State`]. It is plain data with no terminal in it, so
//! what the interface shows can be read, compared and tested without a tty, and
//! the drawing is a function of it and nothing else. That is the rule the rest
//! of this module follows: no widget reads a clock, a file or a channel, and
//! nothing outside the frame writes to the terminal.
//!
//! # The layout
//!
//! Measured from the top down, in constraints:
//!
//! ```text
//! transcript   Fill      the conversation, scrolled by the app
//! input        Min(1)    the draft, the question, or the completion
//! status       Length(1) the model, the dial, the cost, the effort
//! tabs         Length(1) the row, and what the active tab shows
//! suggestion   Length(1) what may be typed next
//! ```
//!
//! Rows counted from the bottom is what the old layout did, and it is where its
//! bugs came from: the chrome reserved what it needed and the conversation lived
//! in whatever was left, so a question one row too tall pushed the box off the
//! screen. A constraint says what it wants and the layout decides.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget as _};

use crate::style::Style;
use crate::terminal::{self, Draw};
use crate::transcript::{Kind, Transcript};

/// What the interface is showing.
///
/// It is the whole of the visible state, and it is deliberately flat: the chrome
/// is a handful of strings that something else computes, and holding them here
/// rather than computing them in the widget is what keeps the widgets from
/// needing the service, the registry or the session.
#[derive(Clone, Debug, Default)]
pub(crate) struct State {
    /// The conversation.
    pub(crate) transcript: Transcript,
    /// The draft being typed, as the editor holds it.
    pub(crate) input: String,
    /// Where the cursor is in the draft, in characters.
    pub(crate) cursor: usize,
    /// The completion, when the line offers one.
    pub(crate) completion: Option<Completion>,
    /// The question the model asked, when it is waiting for an answer.
    pub(crate) question: Option<Question>,
    /// The status row: the model, the dial, the cost, the effort.
    pub(crate) status: String,
    /// The tab row, already rendered by the tabs module.
    pub(crate) tabs: String,
    /// The row under the tabs: what the active tab shows.
    pub(crate) tab_body: String,
    /// What may be typed next, when there is something to say about it.
    pub(crate) suggestion: String,
    /// What the client is waiting for, when a turn is in flight.
    ///
    /// It is what the spinner is drawn from: a value and not a thread, because
    /// the bar used to repaint a line of its own from one, and that is a second
    /// writer on a terminal that has one.
    pub(crate) waiting: Option<crate::render::Wait>,
    /// How many lines the view is scrolled back from the end.
    pub(crate) scroll: usize,
}

/// A completion offered while typing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Completion {
    /// What Tab would insert.
    pub(crate) text: String,
    /// The rows, when there is more than one to show.
    pub(crate) rows: Vec<String>,
    /// Which row is under the cursor.
    pub(crate) cursor: usize,
}

/// The question on screen, waiting for an answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Question {
    /// What was asked.
    pub(crate) prompt: String,
    /// The choices, in the order they were offered.
    pub(crate) choices: Vec<String>,
    /// Which row the cursor is on; one past the last choice is "something
    /// else".
    pub(crate) cursor: usize,
    /// Whether more than one may be taken.
    pub(crate) multiple: bool,
    /// The choices taken so far.
    pub(crate) taken: Vec<usize>,
    /// Whether the free-text box is open.
    pub(crate) typing: bool,
}

impl State {
    /// An empty state, which is what a test draws from.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// How many rows the input takes at this width, one at least.
    ///
    /// The input is as tall as what is in it and no taller than
    /// [`INPUT_ROWS`]; the completion, when open, takes rows from its own
    /// budget rather than growing the block without bound.
    #[must_use]
    fn input_height(&self, width: u16) -> u16 {
        let width = usize::from(width).max(1);
        // A long line is several rows, and the block has to be as tall as what
        // is in it or the draft would be cut off at the bottom of its own
        // space — the words being typed are the last thing that should
        // disappear.
        let wrapped = if self.input.is_empty() {
            1
        } else {
            self.input
                .split('\n')
                .map(|line| line.chars().count().div_ceil(width).max(1))
                .sum::<usize>()
        };
        let completion = self
            .completion
            .as_ref()
            .map_or(0, |completion| completion.rows.len().min(COMPLETION_ROWS));
        let question = usize::from(self.question.is_some());
        u16::try_from(wrapped + completion + question)
            .unwrap_or(INPUT_ROWS)
            .clamp(1, INPUT_ROWS)
    }

    /// Draws one frame into the terminal.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal is gone, which means the process is
    /// ending anyway.
    pub(crate) fn draw(
        &mut self,
        terminal: &mut terminal::Guard,
        style: Style,
    ) -> Result<(), String> {
        terminal.draw(|frame| {
            let area = frame.area();
            let input = self.input_height(area.width);
            let waiting = u16::from(self.waiting.is_some());
            let [conversation, spinner, entry, status, tabs_row, suggestion] = Layout::vertical([
                Constraint::Fill(1),
                Constraint::Length(waiting),
                // The input grows with what is in it and stops: a draft of a
                // thousand lines is not a reason to lose the conversation.
                Constraint::Length(input),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .areas(area);

            draw_transcript(frame, conversation, self, style);
            if waiting > 0 {
                draw_spinner(frame, spinner, self, style);
            }
            draw_input(frame, entry, self, style);
            draw_row(frame, status, &self.status, style.dim_span());
            // The tab row shows which tabs there are and what the active one
            // holds: the names alone would leave a person wondering what the
            // bracket around one of them does, and the body alone would hide
            // that there are others a keystroke away.
            let tabs = if self.tab_body.is_empty() {
                self.tabs.clone()
            } else {
                format!("{}   {}", self.tabs, self.tab_body)
            };
            draw_row(frame, tabs_row, &tabs, style.faint_span());
            draw_row(frame, suggestion, &self.suggestion, style.faint_span());
        })
    }
}

/// The most rows the completion takes from the input block.
pub(crate) const COMPLETION_ROWS: usize = 4;

/// The most rows the input block may take, completion and question included.
///
/// It is a bound and not a reservation: the block is as tall as what is in it,
/// so a draft of one line leaves the rest to the conversation. Past this the
/// draft scrolls inside its own rows.
pub(crate) const INPUT_ROWS: u16 = 10;

/// Draws what the client is waiting for, as a turning glyph.
///
/// It is a wheel of frames advanced by the clock, which is the one animation
/// that earns a clock: it says the process is alive while nothing else on
/// screen changes. The row appears only while a turn is in flight, so a client
/// at a prompt draws exactly as many rows as it has content for.
fn draw_spinner(frame: &mut Draw<'_>, area: Rect, state: &State, style: Style) {
    use std::time::{SystemTime, UNIX_EPOCH};
    if area.height == 0 {
        return;
    }
    // The phase decides the words and the clock decides the glyph: the two say
    // different things and one of them is not about time passing.
    let label = match state.waiting {
        Some(crate::render::Wait::Streaming) => "streaming",
        _ => "waiting for the first token",
    };
    let step = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() / u128::from(SPINNER_MS));
    let glyph = SPINNER[(step % SPINNER.len() as u128) as usize];
    let line = Line::from(vec![
        Span::raw("  "),
        Span::styled(
            glyph.to_owned(),
            style.ratatui((0x56, 0xB6, 0xC2), 44, "36"),
        ),
        Span::raw(" "),
        Span::styled(label.to_owned(), style.dim_span()),
    ]);
    Paragraph::new(line).render(area, frame.buffer_mut());
}

/// How long one glyph of the spinner is shown, in ms.
const SPINNER_MS: u64 = 90;

/// The frames of the spinner, in the order they turn.
///
/// Plain characters rather than braille: every terminal shows them, and a
/// missing glyph would leave a hole where the only sign of life was.
const SPINNER: [&str; 4] = ["◐", "◓", "◑", "◒"];

/// Draws one row of chrome, clipped to its area.
fn draw_row(frame: &mut Draw<'_>, area: Rect, text: &str, style: ratatui::style::Style) {
    if area.height == 0 || text.is_empty() {
        return;
    }
    // A row is one line and the chrome is built to fit it, so what is longer
    // than the row is cut here rather than wrapped: a hint that pushed the
    // input down a row would move everything under the eye mid-keystroke. The
    // builders keep it short — `session_status` drops what does not fit — and
    // this is the backstop for a width they did not expect.
    let room = usize::from(area.width).saturating_sub(2).max(1);
    let text = crate::text::terminal(text);
    let shown = if text.chars().count() > room {
        let mut cut: String = text.chars().take(room.saturating_sub(1)).collect();
        cut.push('…');
        cut
    } else {
        text.into_owned()
    };
    let line = Line::from(vec![Span::raw("  "), Span::styled(shown, style)]);
    Paragraph::new(line).render(area, frame.buffer_mut());
}

/// Draws the conversation: the part that fits, newest at the bottom.
fn draw_transcript(frame: &mut Draw<'_>, area: Rect, state: &State, style: Style) {
    if area.height == 0 {
        return;
    }
    let width = area.width.saturating_sub(2).max(1);
    let lines = transcript_lines(&state.transcript, width);
    // From the bottom up: the newest line is the one being read, and a view
    // that grew from the top would put it off the screen.
    let skipped = state.scroll.min(lines.len());
    let shown = lines.len() - skipped;
    let take = usize::from(area.height).min(shown);
    let from = shown - take;

    // Each kind gets the shape it reads best in: the thought is a box with its
    // own margin, so a run of it is visibly one bounded thing and not the
    // conversation drifting, and everything else is a line.
    let rows: Vec<Line> = lines[from..from + take]
        .iter()
        .map(|(kind, text)| {
            let ink = ink(*kind, style);
            if *kind == Kind::Reasoning {
                // The box's own column, so the eye can tell where the thought
                // starts and ends without reading a word of it.
                return Line::from(vec![
                    Span::raw("  "),
                    Span::styled("│ ", style.faint_span()),
                    Span::styled(text.clone(), ink),
                ]);
            }
            Line::from(vec![Span::raw("  "), Span::styled(text.clone(), ink)])
        })
        .collect();
    // The conversation sits against the input and not against the top: what a
    // person reads is the newest line, and a view pinned to the top row would
    // leave the transcript floating at the top of an empty screen with a gap
    // between it and the prompt.
    let take = u16::try_from(take).unwrap_or(area.height);
    let top = area.y + area.height.saturating_sub(take);
    let inside = Rect::new(area.x, top, area.width, take);
    Paragraph::new(rows).render(inside, frame.buffer_mut());
    let _ = top;
}

/// Every entry of the conversation as the rows a terminal would show.
///
/// The wrap happens here and not in the data: a wrapped line is only correct at
/// the width it was wrapped for, and the width is the terminal's right now.
#[must_use]
pub(crate) fn transcript_lines(transcript: &Transcript, width: u16) -> Vec<(Kind, String)> {
    let mut out = Vec::new();
    for entry in transcript.entries() {
        // A call that is out says so on its own header: the row counts while
        // the work runs, which is the one number that turns a silent wait into
        // something a person can read. It replaces the header rather than
        // adding a line, so nothing under it moves while it counts.
        let counting = entry.kind == Kind::ToolStarted && entry.started.is_some();
        for (index, line) in entry.lines.iter().enumerate() {
            let line = if counting && index == 0 {
                let at = entry.started.unwrap_or_else(std::time::Instant::now);
                format!("{line}  {}", elapsed(at.elapsed()))
            } else {
                line.clone()
            };
            for wrapped in wrap(&line, width) {
                out.push((entry.kind, wrapped));
            }
        }
        // What the bound dropped is said where it was dropped: a cut that hides
        // itself is worse than a short list that admits it.
        if entry.dropped > 0 {
            out.push((
                Kind::Notice,
                format!("… {} earlier line(s) left out", entry.dropped),
            ));
        }
    }
    out
}

/// How long a call that is still out has been running, as the row says it.
///
/// Seconds under a minute and minutes after that: a call that ran past the hour
/// is a call somebody should look at, and the two numbers they need are how long
/// and how many. The ellipsis is what says it is still going.
#[must_use]
pub(crate) fn elapsed(since: std::time::Duration) -> String {
    let seconds = since.as_secs();
    if seconds < 60 {
        format!("{seconds}s…")
    } else if seconds < 3_600 {
        format!("{}m{}s…", seconds / 60, seconds % 60)
    } else {
        format!("{}h{}m…", seconds / 3_600, (seconds % 3_600) / 60)
    }
}

/// Breaks one line to a width, keeping words whole where it can.
#[must_use]
pub(crate) fn wrap(line: &str, width: u16) -> Vec<String> {
    let width = usize::from(width).max(1);
    if line.is_empty() {
        return vec![String::new()];
    }
    let mut rows = Vec::new();
    let mut current = String::new();
    for word in line.split(' ') {
        let size = word.chars().count();
        if current.is_empty() {
            current.push_str(word);
        } else if current.chars().count() + 1 + size <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            rows.push(std::mem::take(&mut current));
            current.push_str(word);
        }
        // A word wider than the row is cut where it stands: moving it would
        // leave the row empty and the next one still too long.
        while current.chars().count() > width {
            rows.push(current.chars().take(width).collect());
            current = current.chars().skip(width).collect();
        }
    }
    if !current.is_empty() {
        rows.push(current);
    }
    if rows.is_empty() {
        rows.push(String::new());
    }
    rows
}

/// The ink one kind of entry is written in.
///
/// Colour is what tells a tool's output from the answer at a glance, and each
/// kind has its own: two that shared one would be two things made to look like
/// the same thing.
#[must_use]
pub(crate) fn ink(kind: Kind, style: Style) -> ratatui::style::Style {
    // Each kind has the ink the line printer gave it, so nothing changes
    // colour as the interface moves. Two kinds that read alike share an ink on
    // purpose: a notice and the model's thought are both asides, and the eye
    // is meant to skip them at the same speed.
    match kind {
        Kind::Said => style.ratatui((0x56, 0xB6, 0xC2), 44, "36"),
        Kind::Answer => ratatui::style::Style::new(),
        Kind::Reasoning | Kind::Notice => style.dim_span(),
        Kind::ToolStarted => style.ratatui((0x61, 0xAF, 0xEF), 39, "34"),
        Kind::ToolOutput => style.faint_span(),
        Kind::Failure => style.ratatui((0xE0, 0x6C, 0x75), 204, "31"),
    }
}

/// Draws the input block: the draft, the completion and the question.
///
/// All three live in the same rows, so they are one widget with three states
/// rather than three widgets competing for the same space.
fn draw_input(frame: &mut Draw<'_>, area: Rect, state: &State, style: Style) {
    if area.height == 0 {
        return;
    }
    let mut rows: Vec<Line> = Vec::new();
    // A question takes the block: it is what the model is waiting on, and the
    // draft is not what the person is working on while it is up.
    if let Some(question) = &state.question {
        rows.push(Line::from(Span::styled(
            crate::text::terminal(&question.prompt).into_owned(),
            style.ratatui((0xE5, 0xC0, 0x7B), 180, "33"),
        )));
        for (index, choice) in question.choices.iter().enumerate() {
            let here = index == question.cursor;
            let taken = question.taken.contains(&index);
            let mark = if question.multiple {
                if taken { "[x] " } else { "[ ] " }
            } else {
                ""
            };
            let text = format!("{} {mark}{choice}", if here { '›' } else { ' ' });
            rows.push(if here {
                Line::styled(text, Style::selected())
            } else {
                Line::from(Span::styled(text, style.dim_span()))
            });
        }
        draw_block(frame, area, &rows);
        return;
    }

    // The draft, wrapped to the width it has and with the cursor drawn as the
    // character it is on. Wrapping is what makes a long line readable: a draft
    // the terminal cut off at the edge would hide the words being typed, which
    // is the one thing the person is looking at.
    let prompt = "› ";
    let width = usize::from(area.width).saturating_sub(prompt.len()).max(1);
    for (index, line) in state.input.split('\n').enumerate() {
        let offset = state
            .input
            .split('\n')
            .take(index)
            .map(|line| line.chars().count() + 1)
            .sum::<usize>();
        let at = state
            .cursor
            .saturating_sub(offset)
            .min(line.chars().count());
        // The line broken into rows of `width` columns, which is what the
        // terminal would otherwise do to it at the edge of the screen.
        let chars: Vec<char> = line.chars().collect();
        let chunks: Vec<&[char]> = if chars.is_empty() {
            vec![&[]]
        } else {
            chars.chunks(width).collect()
        };
        for (index_of_row, part) in chunks.iter().enumerate() {
            let from = index_of_row * width;
            let label = if index == 0 && index_of_row == 0 {
                prompt
            } else {
                "  "
            };
            let mut spans = vec![Span::styled(label, style.faint_span())];
            for (position, character) in part.iter().enumerate() {
                if from + position == at {
                    spans.push(Span::styled(character.to_string(), Style::selected()));
                } else {
                    spans.push(Span::raw(character.to_string()));
                }
            }
            // A cursor at the very end of the line still needs a cell to sit
            // on, or it would be invisible at the one moment it is looked for.
            if at == line.chars().count() && from + part.len() == chars.len() {
                spans.push(Span::styled(" ", Style::selected()));
            }
            rows.push(Line::from(spans));
        }
    }
    // The completion sits under the draft, on the rows the block already has.
    if let Some(completion) = &state.completion {
        for (index, row) in completion.rows.iter().take(COMPLETION_ROWS).enumerate() {
            let here = index == completion.cursor;
            let text = format!("    {row}");
            rows.push(if here {
                Line::styled(text, Style::selected())
            } else {
                Line::from(Span::styled(text, style.dim_span()))
            });
        }
    }
    draw_block(frame, area, &rows);
}

/// Draws rows inside a block, from the top of the area it was given.
fn draw_block(frame: &mut Draw<'_>, area: Rect, rows: &[Line]) {
    let take = usize::from(area.height).min(rows.len());
    let inside = Rect::new(
        area.x,
        area.y,
        area.width,
        u16::try_from(take).unwrap_or(area.height),
    );
    Paragraph::new(rows[..take].to_vec()).render(inside, frame.buffer_mut());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::Entry;

    #[test]
    fn a_long_line_is_wrapped_to_the_width_it_is_given() {
        // The width belongs to the terminal at the moment of the frame, so the
        // wrap is here and not in the data: a wrap kept in the entry would be a
        // wrap for a width the terminal may no longer have.
        let rows = wrap("one two three four", 9);
        assert_eq!(rows, ["one two", "three", "four"]);
    }

    #[test]
    fn a_word_wider_than_the_row_is_cut_rather_than_moved() {
        // Moving it would leave the row empty and the word still too long, so
        // it fills the row and continues on the next one.
        assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
    }

    #[test]
    fn an_empty_line_is_one_empty_row() {
        // A blank line in the conversation is a line, and dropping it would
        // join two paragraphs that were never joined.
        assert_eq!(wrap("", 10), [""]);
    }

    #[test]
    fn a_cut_entry_says_how_much_it_left_out() {
        use crate::transcript::MAX_ENTRY_LINES;
        let mut transcript = Transcript::new();
        let mut entry = Entry::new(Kind::ToolOutput);
        for index in 0..MAX_ENTRY_LINES + 3 {
            entry.push_line(format!("line {index}"));
        }
        transcript.push(entry);
        let lines = transcript_lines(&transcript, 80);
        let last = &lines[lines.len() - 1].1;
        assert!(last.contains('3'), "the count is on screen: {last}");
        assert!(last.contains("left out"), "{last}");
    }

    #[test]
    fn the_conversation_is_read_newest_last() {
        // The view takes from the end, so the order in the data is the order on
        // screen and the last line is the one being read.
        let mut transcript = Transcript::new();
        transcript.push(Entry::line(Kind::Said, "hello"));
        transcript.push(Entry::line(Kind::Answer, "hi"));
        let lines = transcript_lines(&transcript, 80);
        assert_eq!(lines[0], (Kind::Said, "hello".to_owned()));
        assert_eq!(lines[1], (Kind::Answer, "hi".to_owned()));
    }

    #[test]
    fn the_input_grows_with_what_is_in_it_and_stops() {
        // A block that reserved its maximum would take the conversation's rows
        // for a draft of one line.
        let mut state = State::new();
        assert_eq!(state.input_height(80), 1, "an empty draft is one row");
        state.input = "one\ntwo\nthree".to_owned();
        assert_eq!(state.input_height(80), 3);
        // And it stops: a draft of a thousand lines is not a reason to lose the
        // conversation.
        state.input = "x\n".repeat(1_000);
        assert_eq!(state.input_height(80), INPUT_ROWS);
    }

    #[test]
    fn every_kind_of_entry_has_its_own_ink() {
        // Colour is what tells a tool's output from the answer at a glance, and
        // two kinds sharing one would be two things made to look alike.
        let style = Style::for_tests(crate::style::Colors::True);
        // `Notice` and `ToolOutput` are both quiet on purpose: they are the
        // same kind of thing to a reader, which is not the case for the rest.
        let distinct: Vec<ratatui::style::Style> = vec![
            ink(Kind::Said, style),
            ink(Kind::Answer, style),
            ink(Kind::Reasoning, style),
            ink(Kind::ToolStarted, style),
            ink(Kind::Failure, style),
        ];
        for (index, one) in distinct.iter().enumerate() {
            for other in &distinct[index + 1..] {
                assert_ne!(one, other, "two kinds share an ink");
            }
        }
    }
}
