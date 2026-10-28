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

use crate::input::McpRow;
use crate::style::Style;
use crate::terminal::{self, Draw};
use crate::transcript::{Ink, Kind, Transcript};

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
    /// The rows a plugin put beside the conversation, with their plugin's name.
    ///
    /// Empty when nothing was loaded, which is every ordinary run: the column
    /// takes no space when there is nothing to put in it.
    pub(crate) sidebar: Vec<(String, Vec<norte_code_plugins::SidebarRow>)>,
    /// The connected MCP servers, as the sidebar shows them.
    pub(crate) mcp: Vec<McpRow>,
    /// The model's task list, as the sidebar shows it.
    ///
    /// It was a footer tab and a footer row is one line: a plan is a list, and
    /// a list does not fit in a row that also carries the model, the cost and
    /// the effort. It is a column of the sidebar now, where it has the room.
    pub(crate) todos: Vec<TaskRow>,
    /// What may be typed next, when there is something to say about it.
    pub(crate) suggestion: String,
    /// What the client is waiting for, when a turn is in flight.
    ///
    /// It is what the spinner is drawn from: a value and not a thread, because
    /// the bar used to repaint a line of its own from one, and that is a second
    /// writer on a terminal that has one.
    pub(crate) waiting: Option<crate::render::Wait>,
    /// When the wait began, as milliseconds since the epoch.
    ///
    /// It travels from the renderer, which is what sees the event that started
    /// the turn. The frame is what turns it into a number: a duration the frame
    /// computes itself is one it can draw without asking anything.
    pub(crate) waiting_since: Option<u128>,
    /// How many lines the view is scrolled back from the end.
    pub(crate) scroll: usize,
    /// What the mouse has selected, as two corners of the screen.
    ///
    /// The frame draws it and the frame is what reads it back as text: only it
    /// knows what was painted where, and the transcript behind it may have been
    /// wrapped differently for a terminal of another size.
    pub(crate) selection: Option<((u16, u16), (u16, u16))>,
}

/// One row of the task list, as the sidebar draws it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TaskRow {
    /// What the task says.
    pub(crate) text: String,
    /// Whether the model marked it finished.
    pub(crate) done: bool,
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
        // A question takes its own rows: the prompt, every choice, and the row
        // that opens the box. Counting it as one row is what once cut the list
        // to the prompt alone, so the choices never reached the screen.
        let question = self
            .question
            .as_ref()
            .map_or(0, |question| 1 + question.choices.len() + 1);
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
        pinned: &crate::input::Pinned,
    ) -> Result<(), String> {
        terminal.draw(|frame| self.paint(frame, style, pinned))
    }

    /// Paints the whole interface into one frame.
    ///
    /// It is separate from [`State::draw`] so a test can paint into a
    /// `TestBackend` and read the screen back: the layout bugs this file has
    /// had were all visible as the *screen* and invisible in the rows, and the
    /// only way to check the screen is to draw one.
    pub(crate) fn paint(&self, frame: &mut Draw<'_>, style: Style, pinned: &crate::input::Pinned) {
        {
            let area = frame.area();
            let input = self.input_height(area.width);
            let waiting = u16::from(self.waiting.is_some());
            // The screen is divided into two regions and one rule:
            //
            //   ┌──────────────────────────┬──────────────┐
            //   │ conversation             │ sidebar      │
            //   │  (scrolls)               │  tasks       │
            //   │                          │  plugins     │
            //   ├──────────────────────────┤              │
            //   │ waiting                  │              │
            //   │ input                    │              │
            //   │ suggestion               │              │
            //   │ status                   │              │
            //   └──────────────────────────┴──────────────┘
            //
            // The sidebar takes a column on the right, and only when there is
            // something to put in it: the task list, or a plugin that asked for
            // one. A run with neither draws exactly what it drew before there
            // was a sidebar.
            // A sidebar needs a screen to be a sidebar on. At a quarter of a
            // narrow terminal it is a column too thin to read a task in, and
            // what it costs is the conversation, which is the thing the screen
            // is for: below the bound the column is not drawn and the
            // conversation keeps the width.
            let aside =
                (!self.sidebar.is_empty() || !self.todos.is_empty() || !self.mcp.is_empty())
                    && area.width >= SIDEBAR_MIN_WIDTH;
            let body = if aside {
                let [main, rule, side] = Layout::horizontal([
                    Constraint::Fill(1),
                    // One column, drawn as a rule: a panel is a panel because
                    // its edge is visible, and a gap alone reads as a mistake.
                    Constraint::Length(1),
                    Constraint::Percentage(SIDEBAR_PERCENT),
                ])
                .areas(area);
                draw_rule(frame, rule, style);
                draw_sidebar(frame, side, self, style);
                main
            } else {
                area
            };
            // The chrome is measured from the bottom up, and what is left over
            // is the conversation: the input grows with what is typed and the
            // status row is always exactly one line, so the transcript takes
            // whatever those two do not, which is what keeps it scrollable
            // against a terminal of any height.
            let [conversation, spinner, entry, suggestion, rule, status] = Layout::vertical([
                Constraint::Fill(1),
                Constraint::Length(waiting),
                // The input grows with what is in it and stops: a draft of a
                // thousand lines is not a reason to lose the conversation.
                Constraint::Length(input),
                Constraint::Length(1),
                // The rule is a row of its own. It used to be drawn onto the
                // status row and then overwritten by it, so half of it survived
                // as stray dashes across the bottom line.
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .areas(body);

            draw_transcript(frame, conversation, self, style);
            if waiting > 0 {
                draw_spinner(frame, spinner, self, style);
            }
            draw_input(frame, entry, self, style);
            // The suggestion names what may be typed next, and when it names a
            // reasoning setting it means the same thing the bar in the status
            // does: it reads in the same colour, so the scale is learned once.
            draw_row(
                frame,
                suggestion,
                &self.suggestion,
                suggestion_ink(self, style),
            );
            draw_chrome_rule(frame, rule, style);
            // The status closes the frame: it is what the session *is* — the
            // model, the dial, the cost, the effort — and the bottom row is
            // where a status is read from.
            draw_row(frame, status, &self.status, style.dim_span());
        }
        // What was painted, row by row, so a selection can be read back as
        // text. It is taken from the finished frame and not from the state
        // that built it: the frame is what knows how a row was wrapped and
        // where it landed, and the two can disagree after a resize.
        pinned.set_painted(painted_rows(frame));
        if let Some(range) = self.selection {
            highlight(frame, range);
        }
    }
}

/// The text of every row of the frame, as it was drawn.
#[must_use]
fn painted_rows(frame: &mut Draw<'_>) -> Vec<String> {
    let area = frame.area();
    let buffer = frame.buffer_mut();
    (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buffer[(x, y)].symbol().to_owned())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

/// Inverts the cells a selection covers.
///
/// It is an inversion and not a colour: the selection has to be visible over
/// every ink the conversation uses — a green diff line, a grey notice, plain
/// text — and a colour of its own would fight whichever it landed on. Reverse
/// video is what a terminal does with its own selection, so this reads as one.
fn highlight(frame: &mut Draw<'_>, range: ((u16, u16), (u16, u16))) {
    use ratatui::style::Modifier;
    let ((c1, r1), (c2, r2)) = range;
    let (top, left) = if (r1, c1) <= (r2, c2) {
        (r1, c1)
    } else {
        (r2, c2)
    };
    let (bottom, right) = if (r1, c1) <= (r2, c2) {
        (r2, c2)
    } else {
        (r1, c1)
    };
    let area = frame.area();
    let buffer = frame.buffer_mut();
    for row in top..=bottom {
        for column in left..=right {
            if column < area.width && row < area.height {
                buffer[(column, row)].modifier.insert(Modifier::REVERSED);
            }
        }
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

/// Draws what the client is waiting for: a turning glyph and how long.
///
/// The glyph turns in both phases, because both are a wait: a wheel that stopped
/// when tokens started arriving would say the process had stalled at the exact
/// moment it was working. What changes with the phase is the words.
///
/// The number beside it is the time since the wait began, which is the one thing
/// a person watching a silent provider wants and cannot get from anything else
/// on screen.
fn draw_spinner(frame: &mut Draw<'_>, area: Rect, state: &State, style: Style) {
    use std::time::{SystemTime, UNIX_EPOCH};
    if area.height == 0 {
        return;
    }
    // The phase decides the words and the clock decides the glyph: the two say
    // different things and one of them is not about time passing.
    let label = match state.waiting {
        Some(crate::render::Wait::Streaming) => "streaming",
        Some(crate::render::Wait::Thinking) => "thinking",
        _ => "waiting for the first token",
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    let step = now / u128::from(SPINNER_MS);
    let glyph = SPINNER[(step % SPINNER.len() as u128) as usize];
    // The count is of the wait, and the wait began when the client was told a
    // turn started. The frame is the only thing that knows that instant, so it
    // is kept here rather than travelling from the loop.
    let since = state
        .waiting_since
        .map_or(0, |started| now.saturating_sub(started));
    let since_secs = u64::try_from(since / 1_000).unwrap_or(u64::MAX);
    let line = Line::from(vec![
        Span::raw("  "),
        Span::styled(
            glyph.to_owned(),
            style.ratatui((0x56, 0xB6, 0xC2), 44, "36"),
        ),
        Span::raw(" "),
        Span::styled(label.to_owned(), style.dim_span()),
        Span::styled(
            format!(" · {}", elapsed(std::time::Duration::from_secs(since_secs))),
            style.faint_span(),
        ),
    ]);
    Paragraph::new(line).render(area, frame.buffer_mut());
}

/// How long one glyph of the spinner is shown, in ms.
///
/// A live row uses the same clock as the status spinner, so the two turn
/// together: they are one process saying it is alive, not two.
const SPINNER_MS: u64 = 90;

/// The frames of the spinner, in the order they turn.
///
/// Plain characters rather than braille: every terminal shows them, and a
/// missing glyph would leave a hole where the only sign of life was.
const SPINNER: [&str; 4] = ["◐", "◓", "◑", "◒"];

/// The ink the suggestion row is written in.
///
/// A reasoning hint is written in the colour of the rung it names — `xhigh` in
/// the red the bar would draw — and everything else stays quiet, because a hint
/// is an aside and the eye should skip it.
fn suggestion_ink(state: &State, style: Style) -> ratatui::style::Style {
    let hint = state.suggestion.as_str();
    for rung in [
        "xhigh", "max", "high", "medium", "minimal", "low", "off", "auto",
    ] {
        // The hint is `word — setting` or a list of candidates, so the word has
        // to be matched as a word and not found inside another one.
        if hint
            .split(|character: char| !character.is_ascii_alphanumeric())
            .any(|word| word.eq_ignore_ascii_case(rung))
        {
            return crate::render::effort_span(style, rung);
        }
    }
    style.faint_span()
}

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
    // The escapes go first. The chrome is built by the string painters — the
    // status line in particular, which colours each of its fields — and those
    // wrap text in SGR codes. A frame draws text: the codes would be shown as
    // the characters they are, and they would be counted as characters by the
    // cut below, so a row that fit came back chopped mid-field with the tail of
    // an escape sequence left on screen.
    let text = crate::select::strip_style(text);
    let text = crate::text::terminal(&text);
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

/// The bar in front of a row, which is what says who is talking.
///
/// It is a character and not a colour because a terminal with no colour has
/// none: the bar is the part that survives, and the ink on top of it says the
/// same thing twice for a terminal that can show it.
#[must_use]
pub(crate) fn margin(kind: Kind) -> &'static str {
    match kind {
        Kind::Said => "▌ ",
        Kind::Reasoning => "│ ",
        _ => "  ",
    }
}

/// The columns the transcript's margin takes, with its leading space.
///
/// One space and the bar: the narrowest thing that still reads as a column of
/// its own beside the text.
pub(crate) const MARGIN_COLS: u16 = 3;

/// Which part of a box a row is.
///
/// A tool's output is one bounded thing, and the frame is what says so. The
/// frame is drawn by the view and is not part of the row's text: a box whose
/// edges were rows of the conversation would be a box nobody can select the
/// inside of without taking the edges with them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Frame {
    /// The rule that opens the box, carrying its title.
    Top,
    /// A row between the rules.
    Inside,
    /// The rule that closes it.
    Bottom,
}

/// One row of the conversation, as the view draws it.
///
/// It is the wrap of one [`crate::transcript::Line`] at the width of the
/// terminal, with the frame the row belongs to and the ink it is written in
/// carried separately. Nothing here is a string of escape codes: the widget
/// draws text, and a code kept in the text is what put `38;2;97;175;239m` on
/// screen where a colour was meant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Row {
    /// The ink, when the row's own is not the one its kind asks for.
    pub(crate) ink: Option<Ink>,
    /// What kind of entry the row belongs to.
    pub(crate) kind: Kind,
    /// The text of this row, already wrapped.
    ///
    /// It is what the row *says*, with no markup in it: a rank of a list and a
    /// bold word are text here and emphasis in `runs`. It is also what a
    /// selection copies, and what the tests assert on.
    pub(crate) text: String,
    /// Which part of a box it is, for an entry the view frames.
    pub(crate) frame: Option<Frame>,
    /// The runs the row is drawn from, when it is more than one ink.
    ///
    /// Empty for a row that is one piece of text, which is most of them: a
    /// tool's output, a notice, a message. A model's answer is markdown, and it
    /// arrives with a heading, a list and a fenced block in it, so it is a run
    /// per emphasis — and the view is what turns an emphasis into a colour.
    pub(crate) runs: Vec<crate::markdown::Run>,
}

/// Draws the conversation: the part that fits, newest at the bottom.
fn draw_transcript(frame: &mut Draw<'_>, area: Rect, state: &State, style: Style) {
    if area.height == 0 {
        return;
    }
    // What the text has is what is left after the margin every row carries:
    // wrapping at the full width would put the last word of a row past the edge
    // of the terminal, where it is cut.
    let width = area.width.saturating_sub(MARGIN_COLS).max(1);
    let lines = transcript_lines(&state.transcript, width);
    // From the bottom up: the newest line is the one being read, and a view
    // that grew from the top would put it off the screen. The frame is what
    // clamps the offset, because it is the only thing that knows how many rows
    // fit: the reader moves a number of lines and never knows the height.
    let (from, take) = viewport(lines.len(), area.height, state.scroll);

    // The conversation sits against the input and not against the top: what a
    // person reads is the newest line, and a view pinned to the top row would
    // leave the transcript floating at the top of an empty screen with a gap
    // between it and the prompt.
    //
    // The rows that precede it are drawn as blanks rather than left alone. The
    // area is fixed and the content is not: a frame that painted only the rows
    // it had moved the rect down the screen as the conversation grew, and what
    // stayed above it was whatever the last frame had put there — which is what
    // left the tail of an older answer interleaved with the new one on a real
    // terminal.
    let blank = usize::from(area.height).saturating_sub(take);
    let mut rows: Vec<Line> = Vec::with_capacity(usize::from(area.height));
    rows.extend(std::iter::repeat_with(Line::default).take(blank));
    rows.extend(
        lines[from..from + take]
            .iter()
            .map(|row| row_line(row, area.width, style)),
    );
    Paragraph::new(rows).render(area, frame.buffer_mut());
}

/// The rows of the conversation a view of `height` rows shows, and where.
///
/// The offset counts from the end, which is what makes the view usable while an
/// answer streams in: a person reading history stays on the line they were on,
/// and one at the tail sees the newest line because zero is the tail. The frame
/// is what clamps, because it is the only thing that knows how many rows fit.
#[must_use]
pub(crate) fn viewport(total: usize, height: u16, scroll: usize) -> (usize, usize) {
    let height = usize::from(height);
    let take = height.min(total);
    // Scrolling back can go until the first row is the first row on screen and
    // no further. Clamping to the whole conversation instead was a scroll off
    // its end: `total - total` is nothing to draw, so holding a page key down
    // emptied the screen and left no line to scroll back from.
    let skipped = scroll.min(total - take);
    (total - take - skipped, take)
}

/// One row, with its margin, its frame and its ink.
///
/// The frame is chrome the view adds, and it is deliberately two rules and no
/// vertical bars: a box drawn with sides puts a `│` on every row of the text it
/// holds, and those bars are what a person selects along with the output they
/// meant to copy. A rule above and a rule below say where the box starts and
/// ends and leave every row of the content clean.
fn row_line(row: &Row, width: u16, style: Style) -> Line<'static> {
    let ink = row.ink.map_or_else(
        || ink(row.kind, style),
        |ink| match ink {
            Ink::Faint => style.faint_span(),
            Ink::Dim => style.dim_span(),
            Ink::Cyan => style.ratatui((0x56, 0xB6, 0xC2), 44, "36"),
            Ink::Green => style.ratatui((0x98, 0xC3, 0x79), 71, "32"),
            Ink::Red => style.ratatui((0xE0, 0x6C, 0x75), 203, "31"),
            Ink::Yellow => style.ratatui((0xE5, 0xC0, 0x7B), 180, "33"),
        },
    );
    match row.frame {
        Some(Frame::Top) => {
            // The title leads, and the rule fills what is left of the row.
            let label = format!("─ {} ", row.text);
            let room = usize::from(width).saturating_sub(3);
            let label: String = label.chars().take(room).collect();
            let fill = room.saturating_sub(label.chars().count());
            Line::from(vec![
                Span::raw("  "),
                Span::styled("╭", style.blue_span()),
                Span::styled(label.clone(), Style::bold_span()),
                Span::styled("─".repeat(fill), style.blue_span()),
                Span::styled("╮", style.blue_span()),
            ])
        }
        Some(Frame::Bottom) => Line::from(vec![
            Span::raw("  "),
            Span::styled("╰", style.blue_span()),
            Span::styled(
                "─".repeat(usize::from(width).saturating_sub(2)),
                style.blue_span(),
            ),
            Span::styled("╯", style.blue_span()),
        ]),
        // Inside a box the text keeps its own indent and nothing else: the row
        // is the output, and a bar in front of it is a bar in the copy.
        Some(Frame::Inside) => {
            Line::from(vec![Span::raw("  "), Span::styled(row.text.clone(), ink)])
        }
        None => {
            // The margin is what says who is talking. One column, before every
            // line: the person at `▌` and the model at a space, which is the
            // convention a chat and a diff already share. A colour alone cannot
            // carry it — a terminal with no colour shows none — and the margin
            // costs a column and reads at a glance.
            let bar_ink = match row.kind {
                Kind::Said => style.ratatui((0x56, 0xB6, 0xC2), 44, "36"),
                Kind::Reasoning => style.faint_span(),
                _ => style.dim_span(),
            };
            let bar = margin(row.kind);
            let mut spans = vec![Span::raw(" "), Span::styled(bar.to_owned(), bar_ink)];
            spans.extend(row_spans(row, ink, style));
            Line::from(spans)
        }
    }
}

/// The spans one row's text is drawn from.
///
/// A row that is one piece of text is one span, which is most of them. A row of
/// a model's answer is a run per emphasis, and each run is the row's own ink
/// with the emphasis on top: a heading is the answer's colour and bold, not a
/// colour of its own — the answer is still the answer.
fn row_spans(row: &Row, ink: ratatui::style::Style, style: Style) -> Vec<Span<'static>> {
    use ratatui::style::Modifier;
    if row.runs.is_empty() {
        return vec![Span::styled(row.text.clone(), ink)];
    }
    row.runs
        .iter()
        .map(|run| {
            let mut draw = ink;
            match run.emphasis {
                // Bold and italic are modifiers and not colours: the run keeps
                // the ink of the thing it belongs to, so a bold word inside an
                // answer reads as an emphasised answer and not as a label.
                crate::markdown::Emphasis::Bold => draw = draw.add_modifier(Modifier::BOLD),
                crate::markdown::Emphasis::Italic => draw = draw.add_modifier(Modifier::ITALIC),
                crate::markdown::Emphasis::Heading => {
                    draw = draw.add_modifier(Modifier::BOLD).fg(style.cyan_color());
                }
                // A code span is its own ink in every terminal: it is a
                // different kind of thing from the prose around it, and a
                // modifier alone would not say so on a terminal that renders
                // none.
                // A code span and a list's bullet are the same ink: both are
                // punctuation the reader is meant to tell apart from the prose
                // at a glance, and neither is prose.
                crate::markdown::Emphasis::Code | crate::markdown::Emphasis::Bullet => {
                    draw = draw.fg(style.cyan_color());
                }
                // A quote and a fenced block are both text a person reads
                // past, so they share the quiet ink the rest of the client
                // gives an aside.
                crate::markdown::Emphasis::Quote | crate::markdown::Emphasis::Fence => {
                    draw = draw.fg(style.muted_color());
                }
                crate::markdown::Emphasis::Link => draw = draw.fg(style.blue_color()),
                // A strike is the line-through and nothing else: it is an
                // emphasis on the text, not a different kind of text.
                crate::markdown::Emphasis::Strike => {
                    draw = draw.add_modifier(Modifier::CROSSED_OUT);
                }
                // An underline keeps the ink and adds the line under it, the
                // way bold keeps the ink and adds weight.
                crate::markdown::Emphasis::Underline => {
                    draw = draw.add_modifier(Modifier::UNDERLINED);
                }
                // A table's header is its column names: bold, in the answer's
                // own ink, so it reads as a label and not as another colour.
                crate::markdown::Emphasis::TableHead => {
                    draw = draw.add_modifier(Modifier::BOLD);
                }
                // A body cell is ordinary text: the padding and the `│` between
                // cells are what draw the table, and a colour of its own would
                // fight the prose around it. So it takes the same arm as plain
                // text, which is what it is.
                crate::markdown::Emphasis::TableCell | crate::markdown::Emphasis::Plain => {}
            }
            Span::styled(run.text.clone(), draw)
        })
        .collect()
}

/// Every entry of the conversation as the rows a terminal would show.
///
/// The wrap happens here and not in the data: a wrapped line is only correct at
/// the width it was wrapped for, and the width is the terminal's right now.
#[must_use]
pub(crate) fn transcript_lines(transcript: &Transcript, width: u16) -> Vec<Row> {
    let mut out = Vec::new();
    for entry in transcript.entries() {
        // A boxed entry opens with its rule, whatever it holds: the rule is what
        // says where the output begins, and an entry with no rows at all is one
        // the rule and the closing rule already describe.
        if entry.boxed {
            out.push(Row {
                ink: None,
                kind: entry.kind,
                text: entry.title.clone().unwrap_or_default(),
                frame: Some(Frame::Top),
                runs: Vec::new(),
            });
        }
        // A model's answer is markdown, and it is rendered as a whole and not a
        // line at a time: a fence spans lines, so a renderer that saw one line
        // would draw the code as prose and the prose as code. Everything else is
        // text — a command's bytes, a notice's sentence — and a `**` in one of
        // those is two asterisks.
        //
        // The render is where the width is known and where an emphasis becomes a
        // run the widget draws: the data holds what arrived, and how it is drawn
        // is the view's.
        if entry.kind == Kind::Answer {
            for rendered in crate::markdown::render(
                &entry
                    .lines
                    .iter()
                    .map(|line| line.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
            ) {
                push_rendered(&mut out, entry.kind, &rendered, width);
            }
            if entry.dropped > 0 {
                out.push(Row {
                    ink: Some(Ink::Yellow),
                    kind: Kind::Notice,
                    text: format!("… {} earlier line(s) left out", entry.dropped),
                    frame: None,
                    runs: Vec::new(),
                });
            }
            continue;
        }
        for (index, line) in entry.lines.iter().enumerate() {
            // A call that is out says so on the row it will end on: a glyph
            // that turns and the time it has taken. The clock is the one number
            // that turns a silent wait into something a person can read, and the
            // glyph is what says the process is alive — a row that only counted
            // seconds looks the same as one that stopped counting.
            //
            // It is drawn here and not written into the row: the glyph turns
            // between frames, and a row that carried one would be a row the
            // data kept rewriting.
            let counting = entry.kind == Kind::ToolStarted && entry.started.is_some();
            // The escapes go first, for the same reason they go in `draw_row`:
            // some entries are built by the string painters, and a frame draws
            // text. They also count as characters in the wrap, so a line that
            // fitted was re-flowed around codes the reader never sees.
            let text = if counting && index == 0 {
                let at = entry.started.unwrap_or_else(std::time::Instant::now);
                format!("{}  {}", line.text, elapsed(at.elapsed()))
            } else {
                line.text.clone()
            };
            let text = crate::select::strip_style(&text);
            let text = crate::text::terminal(&text).into_owned();
            // Wrapped to the width the box's inside has, which is the text
            // width and not the row width: the frame's rules take columns of
            // their own.
            let frame = entry.boxed.then_some(Frame::Inside);
            // The turning glyph leads the row that is counting, and only that
            // row: a finished call carries its own mark — the tick, the cross —
            // which the ending wrote into the text.
            let text = if counting && index == 0 {
                let step = (std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |since| since.as_millis())
                    / u128::from(SPINNER_MS)) as usize;
                format!("{} {text}", SPINNER[step % SPINNER.len()])
            } else {
                text
            };
            let ink = if counting && index == 0 {
                Some(Ink::Dim)
            } else {
                line.ink
            };
            let wrapped = wrap(&text, width);
            for text in wrapped {
                out.push(Row {
                    ink,
                    kind: entry.kind,
                    text,
                    frame,
                    runs: Vec::new(),
                });
            }
        }
        // What the bound dropped is said where it was dropped: a cut that hides
        // itself is worse than a short list that admits it.
        if entry.dropped > 0 {
            out.push(Row {
                ink: Some(Ink::Yellow),
                kind: Kind::Notice,
                text: format!("… {} earlier line(s) left out", entry.dropped),
                frame: entry.boxed.then_some(Frame::Inside),
                runs: Vec::new(),
            });
        }
        if entry.boxed {
            out.push(Row {
                ink: None,
                kind: entry.kind,
                text: String::new(),
                frame: Some(Frame::Bottom),
                runs: Vec::new(),
            });
        }
    }
    out
}

/// Pushes one rendered markdown line as the rows it wraps to.
///
/// The indent is drawn as leading spaces inside every row the line wraps to, so
/// a list item's continuation lines up under its text and not at the margin: a
/// wrapped bullet whose second row started at the left would read as a second
/// item.
///
/// The runs are split at the wrap and rebuilt per row, so a bold word that
/// straddles the break is bold on both sides of it. What is carried per row is
/// the emphasis of each piece, which is what the widget draws.
fn push_rendered(out: &mut Vec<Row>, kind: Kind, rendered: &crate::markdown::Rendered, width: u16) {
    let indent = rendered.indent.min(crate::markdown::max_indent());
    let pad = " ".repeat(indent);
    let room = usize::from(width).saturating_sub(indent).max(1);
    // The runs are laid out into physical rows first, so a break can land in
    // the middle of a run and the two halves keep its emphasis.
    let mut rows: Vec<Vec<crate::markdown::Run>> = vec![Vec::new()];
    let mut used = 0usize;
    for run in &rendered.runs {
        let mut piece = String::new();
        for word in run.text.split_inclusive(' ') {
            let size = word.chars().count();
            if used > 0 && used + size > room {
                if !piece.is_empty() {
                    rows.last_mut().expect("a row").push(crate::markdown::Run {
                        text: std::mem::take(&mut piece),
                        emphasis: run.emphasis,
                    });
                }
                rows.push(Vec::new());
                used = 0;
            }
            piece.push_str(word);
            used += size;
        }
        if !piece.is_empty() {
            rows.last_mut().expect("a row").push(crate::markdown::Run {
                text: piece,
                emphasis: run.emphasis,
            });
        }
    }
    for runs in rows {
        // Every row of a wrapped line is indented, not only the first: a
        // continuation that started at the margin would read as a new item.
        let mut whole = runs;
        if !pad.is_empty() {
            whole.insert(0, crate::markdown::Run::plain(pad.clone()));
        }
        let text: String = whole.iter().map(|run| run.text.as_str()).collect();
        out.push(Row {
            ink: None,
            kind,
            text,
            frame: None,
            runs: whole,
        });
    }
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
///
/// Indentation is preserved. It used to be lost: the split was on `' '` and the
/// empty runs that leading spaces produce were dropped, so `    let x = 1;` came
/// back as `let x = 1;`. For a diff that is the whole shape of the thing — a
/// removed line and the line it became are read by their indent — and for a
/// command's output it is the structure a tree or a JSON blob is read by.
#[must_use]
pub(crate) fn wrap(line: &str, width: u16) -> Vec<String> {
    let width = usize::from(width).max(1);
    if line.is_empty() {
        return vec![String::new()];
    }
    // Tabs are already spaces by the time a row reaches here: the transcript
    // expands them when the line arrives, because the width of a tab depends on
    // the column it starts at, and that column is not known here.
    //
    // The leading spaces are kept, and so is a run of them inside the line. The
    // split used to be on `' '` with the empty runs it produces dropped, which
    // lost the indent silently: `    let x = 1;` came back flush against the
    // margin. For a diff that is the whole shape of the thing, and for a
    // command's output it is the structure a tree or a JSON blob is read by.
    let indent = line.len() - line.trim_start_matches(' ').len();
    let lead: String = line.chars().take(indent).collect();
    let body = &line[indent..];
    let mut rows: Vec<String> = Vec::new();
    let mut current = String::new();
    current.push_str(&lead);
    for piece in pieces(body) {
        // A run of spaces is kept when it fits. When it does not, the row is
        // full, and a break is where a space goes anyway.
        if piece.chars().all(|character| character == ' ') {
            if current.chars().count() + piece.chars().count() <= width {
                current.push_str(piece);
            }
            continue;
        }
        if current.chars().count() + piece.chars().count() <= width {
            current.push_str(piece);
        } else {
            // A row holding nothing but the indent is not a row: pushing it
            // would put a blank line under a word too long for the width.
            if !current.trim_end().is_empty() {
                rows.push(current.trim_end().to_owned());
            }
            // A wrapped continuation is indented under the line it belongs to,
            // which is what makes a wrapped paragraph read as one paragraph.
            current.clear();
            current.push_str(&lead);
            current.push_str(piece);
        }
        // A word wider than the row is cut where it stands: moving it would
        // leave the row empty and the next one still too long.
        while current.chars().count() > width {
            rows.push(current.chars().take(width).collect());
            current = current.chars().skip(width).collect();
        }
    }
    if !current.is_empty() {
        rows.push(current.trim_end().to_owned());
    }
    if rows.is_empty() {
        rows.push(String::new());
    }
    rows
}

/// One line cut into the runs a wrap works on: words, and the spaces between.
///
/// A run is maximal, so two spaces together are one piece and stay two spaces.
/// Splitting on `' '` and treating every part as a word is what lost them.
fn pieces(body: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_space: Option<bool> = None;
    for (at, character) in body.char_indices() {
        let space = character == ' ';
        match in_space {
            None => in_space = Some(space),
            Some(seen) if seen != space => {
                out.push(&body[start..at]);
                start = at;
                in_space = Some(space);
            }
            _ => {}
        }
    }
    if start < body.len() {
        out.push(&body[start..]);
    }
    out
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

/// Cuts plain text to a number of columns, saying that it did.
///
/// It is for text a plugin wrote: the row has as many columns as the layout
/// gave it and no more, and a row that ran past the edge would be cut by the
/// terminal at a place nobody chose.
#[must_use]
fn clip_visible(text: &str, columns: usize) -> String {
    if text.chars().count() <= columns {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(columns.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// How much of the width the sidebar takes.
///
/// A quarter, and it is a bound on what the conversation loses: a sidebar that
/// grew with its content would take the screen from the thing it is beside.
const SIDEBAR_PERCENT: u16 = 25;

/// The narrowest terminal the sidebar is drawn on.
///
/// Under it a quarter of the width is a column too thin to read a task in, and
/// the conversation is what the screen is for: the sidebar gives way, and the
/// row it would have shown is still reachable with `/jobs`, `/tools` and the
/// task events in the conversation itself.
const SIDEBAR_MIN_WIDTH: u16 = 72;

/// Draws the one column between the conversation and the sidebar.
///
/// A panel is a panel because its edge is visible: a gap alone reads as a
/// mistake in the layout, and a rule reads as the border it is.
fn draw_rule(frame: &mut Draw<'_>, area: Rect, style: Style) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let rows: Vec<Line> = (0..area.height)
        .map(|_| {
            Line::from(Span::styled(
                "│",
                style.ratatui((0x4A, 0x4E, 0x54), 238, "90"),
            ))
        })
        .collect();
    Paragraph::new(rows).render(area, frame.buffer_mut());
}

/// Draws a horizontal rule across the top edge of an area.
fn draw_chrome_rule(frame: &mut Draw<'_>, area: Rect, style: Style) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let line = Line::from(Span::styled(
        "─".repeat(usize::from(area.width)),
        style.ratatui((0x4A, 0x4E, 0x54), 238, "90"),
    ));
    let strip = Rect::new(area.x, area.y, area.width, 1);
    Paragraph::new(line).render(strip, frame.buffer_mut());
}

/// Draws the column a plugin asked for, beside the conversation.
///
/// It is drawn in the area the layout gave it and nowhere else, and every row
/// is clipped to the width: a plugin says what to read and the host decides how
/// much of it fits. A plugin that asked for more rows than there are gets the
/// beginning of its list and a count of what did not fit — the rule every
/// bounded list in this client follows.
fn draw_sidebar(frame: &mut Draw<'_>, area: Rect, state: &State, style: Style) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let width = area.width;
    // The panels the column holds, top to bottom. The task list leads: it is
    // what the model is doing, and a person watching the work wants to see it
    // before they want what a plugin has to say about it.
    let mut panels: Vec<(String, Vec<Line>)> = Vec::new();
    if !state.todos.is_empty() {
        let done = state.todos.iter().filter(|task| task.done).count();
        let rows = state
            .todos
            .iter()
            .map(|task| {
                // The mark is a character and not a colour: a terminal with no
                // colour shows none, and the state of a task is the one thing
                // this column exists to say.
                let (mark, ink) = if task.done {
                    ("✔", style.faint_span())
                } else {
                    ("·", style.dim_span())
                };
                let text = format!(
                    "{mark} {}",
                    clip_visible(
                        &crate::text::terminal(&task.text),
                        usize::from(width).saturating_sub(3)
                    )
                );
                Line::from(Span::styled(text, ink))
            })
            .collect();
        panels.push((format!("tasks {done}/{}", state.todos.len()), rows));
    }
    if !state.mcp.is_empty() {
        let on = state.mcp.iter().filter(|row| row.active).count();
        let rows = state
            .mcp
            .iter()
            .map(|row| {
                // The state is a word and not only a colour: a terminal with no
                // colour shows none, and whether a server is on is what this row
                // is for.
                let (mark, ink) = if row.active {
                    ("on ", style.dim_span())
                } else {
                    ("off", style.faint_span())
                };
                let text = format!(
                    "{mark} {} ({})",
                    clip_visible(
                        &crate::text::terminal(&row.name),
                        usize::from(width).saturating_sub(14)
                    ),
                    row.tools
                );
                Line::from(Span::styled(text, ink))
            })
            .collect();
        panels.push((format!("mcp {on}/{}", state.mcp.len()), rows));
    }
    for (plugin, list) in &state.sidebar {
        // The name leads, because a column of rows with no attribution is one
        // nobody can turn off.
        let rows = list
            .iter()
            .map(|row| {
                let ink = if row.heading {
                    Style::bold_span()
                } else {
                    style.dim_span()
                };
                Line::from(Span::styled(
                    clip_visible(&crate::text::terminal(&row.text), usize::from(width)),
                    ink,
                ))
            })
            .collect();
        panels.push((plugin.clone(), rows));
    }
    let mut rows: Vec<Line> = Vec::new();
    for (index, (title, body)) in panels.iter().enumerate() {
        if index > 0 {
            rows.push(Line::default());
        }
        // Each panel has a heading and a rule under it: a column of rows with
        // no edge is a column nobody can tell apart from the conversation.
        rows.push(Line::from(Span::styled(
            clip_visible(title, usize::from(width)),
            style.faint_span(),
        )));
        rows.push(Line::from(Span::styled(
            "─".repeat(usize::from(width)),
            style.ratatui((0x4A, 0x4E, 0x54), 238, "90"),
        )));
        rows.extend(body.iter().cloned());
    }
    let take = usize::from(area.height).min(rows.len());
    let hidden = rows.len().saturating_sub(take);
    let mut shown = rows[..take].to_vec();
    if hidden > 0 {
        if let Some(last) = shown.last_mut() {
            *last = Line::from(Span::styled(
                format!("… {hidden} more"),
                style.yellow_span(),
            ));
        }
    }
    Paragraph::new(shown).render(area, frame.buffer_mut());
}

/// Draws a question: the prompt, every choice, the row that opens the box, and
/// the box itself once it is open.
///
/// It is its own function because a question replaces the block rather than
/// growing it, and folding it into the draft's drawing made one function do two
/// jobs. The prompt is the model's own words and keeps the amber ink the wait
/// uses, so a question reads as the model speaking and not as a notice.
fn draw_question(
    frame: &mut Draw<'_>,
    area: Rect,
    question: &Question,
    state: &State,
    style: Style,
) {
    let mut rows: Vec<Line> = Vec::new();
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
    // The row that opens the box is not a choice and is drawn last, even when the
    // model offered nothing: a question with no choices still has to be
    // answerable, and this is the row that makes it so.
    {
        let here = question.cursor >= question.choices.len();
        let text = format!(
            "{} {}",
            if here { '›' } else { ' ' },
            if question.typing {
                "something else:"
            } else {
                "something else"
            }
        );
        rows.push(if here {
            Line::styled(text, Style::selected())
        } else {
            Line::from(Span::styled(text, style.dim_span()))
        });
    }
    // The box, once "something else" was chosen: the answer is typed at the
    // prompt. Before that no letter belongs here, so nothing is drawn for it.
    if question.typing {
        let text = state.input.split('\n').next().unwrap_or_default();
        rows.push(Line::from(Span::styled(
            format!("> {text}"),
            style.dim_span(),
        )));
    }
    draw_block(frame, area, &rows);
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
        draw_question(frame, area, question, state, style);
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
    use crate::input::Pinned;
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
    fn indentation_survives_the_wrap() {
        // A diff's shape is its indent: the line that left and the line that
        // arrived are read by how far in they start. Dropping the leading spaces
        // was silent — the row simply came back flush against the margin — and
        // it made every wrapped diff and every nested line of output lie.
        assert_eq!(wrap("    let x = 1;", 40), ["    let x = 1;"]);
        // A wrapped continuation stays indented under the line it came from, so
        // a wrapped paragraph still reads as one paragraph.
        assert_eq!(
            wrap("    one two three", 10),
            ["    one", "    two", "    three"],
            "the continuation is indented too"
        );
        // And a run of spaces inside the line is not collapsed into one.
        assert_eq!(wrap("a  b", 20), ["a  b"]);
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
        let last = &lines[lines.len() - 1].text;
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
        assert_eq!(lines[0].kind, Kind::Said);
        assert_eq!(lines[0].text, "hello");
        assert_eq!(lines[1].kind, Kind::Answer);
        assert_eq!(lines[1].text, "hi");
    }

    #[test]
    fn a_boxed_entry_draws_its_rules_and_carries_no_bars_inside() {
        // The frame says where the output begins and ends. What it must not do
        // is put a bar on every row of the content: those bars are what a person
        // selects along with the output they meant to copy, and a box drawn with
        // sides is a box whose inside cannot be copied cleanly.
        let mut transcript = Transcript::new();
        let mut entry = Entry::bordered(Kind::ToolOutput, "shell #abc");
        entry.push_line("one");
        entry.push_line("two");
        transcript.push(entry);
        let rows = transcript_lines(&transcript, 80);
        assert_eq!(rows[0].frame, Some(Frame::Top));
        assert_eq!(rows[0].text, "shell #abc");
        assert_eq!(rows[1].frame, Some(Frame::Inside));
        assert_eq!(rows[1].text, "one");
        assert_eq!(rows[2].frame, Some(Frame::Inside));
        assert_eq!(rows[3].frame, Some(Frame::Bottom));
        for row in &rows[1..4] {
            assert!(
                !row.text.contains('│'),
                "the content carries no bar: {:?}",
                row.text
            );
        }
    }

    #[test]
    fn no_row_the_view_draws_carries_an_escape_sequence() {
        // The bug this design exists to prevent: the rows were built by the
        // string painters, which wrap text in raw SGR codes, and the frame put
        // the codes on screen as the characters they are. A row is text and an
        // ink, and the ink is what the widget turns into colour.
        let mut transcript = Transcript::new();
        let mut diff = Entry::bordered(Kind::ToolOutput, "edit 1/1 · src/main.rs");
        diff.push_inked(Ink::Red, "- old");
        diff.push_inked(Ink::Green, "+ new");
        transcript.push(diff);
        for row in transcript_lines(&transcript, 80) {
            assert!(
                !row.text.contains('\u{1b}'),
                "a row is text and an ink, never an escape: {:?}",
                row.text
            );
        }
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
    fn a_new_line_does_not_move_what_is_being_read() {
        // The offset counts from the end, which is what makes the view usable
        // while an answer streams in: a person reading history stays on the
        // line they were on, and one at the tail sees the newest line because
        // zero is the tail.
        let mut transcript = Transcript::new();
        for index in 0..40 {
            transcript.push(Entry::line(Kind::Answer, format!("line {index}")));
        }
        let width = 80;
        // Twenty rows of a hundred, three lines back: the window is fixed to
        // its offset from the end, wherever the end is.
        let before = transcript_lines(&transcript, width);
        let from_before = before.len() - 3;
        assert_eq!(before[from_before - 20].text, "line 17", "twenty rows back");
        transcript.push(Entry::line(Kind::Answer, "one more".to_owned()));
        let after = transcript_lines(&transcript, width);
        let from_after = after.len() - 3;
        assert_eq!(
            after[from_after - 20].text,
            "line 18",
            "and the same rows are on screen one line further in"
        );
    }

    /// The frame a state draws, as lines of text.
    ///
    /// It is the whole interface at a size the test chose, which is the one
    /// thing the unit tests above cannot see: they check what the rows *are*
    /// and not what the screen *shows*. Every layout bug this file has had —
    /// the rule overwritten by the status, the panel that took the wrong
    /// columns, the conversation painted over its own tail — was invisible to
    /// them and obvious here.
    fn frame_of(state: &State, width: u16, height: u16) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        let state = state.clone();
        terminal
            .draw(|frame| {
                let area = frame.area();
                let body = Rect::new(area.x, area.y, area.width, area.height);
                draw_transcript(frame, body, &state, Style::plain());
            })
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    /// Draws twice into the same terminal and reports the second frame.
    ///
    /// It is the one thing a single-frame test cannot see: a frame is diffed
    /// against the last one, so a row that used to hold more than it does now
    /// keeps the tail of the old text unless the frame writes over it.
    fn second_frame(first: &State, then: &State, width: u16, height: u16) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        for state in [first, then] {
            let state = state.clone();
            terminal
                .draw(|frame| {
                    let area = frame.area();
                    draw_transcript(frame, area, &state, Style::plain());
                })
                .expect("draw");
        }
        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    /// The whole interface drawn twice, as the screen the second frame shows.
    fn whole_frames(build: impl Fn(&mut State), then: impl Fn(&mut State)) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let (w, h) = (100u16, 32u16);
        let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("terminal");
        let pinned = Pinned::new();
        for step in [&build as &dyn Fn(&mut State), &then] {
            let mut state = State::new();
            step(&mut state);
            terminal
                .draw(|frame| state.paint(frame, Style::plain(), &pinned))
                .expect("draw");
        }
        let buffer = terminal.backend().buffer().clone();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn a_frame_that_shrinks_leaves_nothing_of_the_one_before_it() {
        // What this is for: on a real terminal a long answer followed by a
        // shorter one left the tail of the first sitting beside the second, and
        // a box rule that moved up left its dashes behind on the old row. A
        // frame is diffed against the last one, so a cell the new frame does
        // not write keeps what the old frame put there.
        let long = |state: &mut State| {
            state.transcript.push(Entry::line(
                Kind::Answer,
                "a very long line that fills the row and then keeps going past its end",
            ));
            for n in 0..12 {
                state.transcript.push(Entry::line(
                    Kind::Answer,
                    format!("line {n} of a longer answer"),
                ));
            }
        };
        let short = |state: &mut State| {
            state.transcript.push(Entry::line(Kind::Answer, "short"));
        };
        let rows = whole_frames(long, short);
        // Every row that is not the answer is blank: no dash, no word of the
        // answer that was there before.
        // The chrome is written every frame and belongs where it is: the input
        // prompt, and the rule that closes the frame under it.
        let chrome =
            |row: &str| row.trim_start().starts_with('›') || row.trim_start().starts_with('─');
        for (at, row) in rows.iter().enumerate() {
            let trimmed = row.trim();
            if trimmed.is_empty() || chrome(row) {
                continue;
            }
            assert!(
                trimmed.contains("short"),
                "row {at} holds something the last frame left: {row:?}"
            );
        }
    }

    #[test]
    fn a_shorter_line_does_not_leave_the_tail_of_the_longer_one() {
        // The bug this exists for: a row that held a long line keeps whatever
        // the new, shorter line did not reach. On a real terminal it came out as
        // stray dashes from a box rule and the tail of an older answer sitting
        // beside the new text.
        let mut first = State::new();
        first.transcript.push(Entry::line(
            Kind::Answer,
            "a line long enough to reach the right edge of the view",
        ));
        let mut then = State::new();
        then.transcript.push(Entry::line(Kind::Answer, "short"));
        let rows = second_frame(&first, &then, 40, 4);
        let painted = rows
            .iter()
            .find(|row| !row.trim().is_empty())
            .expect("a row");
        assert!(
            painted.trim_end().ends_with("short"),
            "the tail of the long line is still there: {rows:?}"
        );
    }

    /// Paints a frame and hands back the pinned handle it wrote to.
    ///
    /// The selection is read from what the frame painted, so the only way to
    /// test it is to paint one: a test that built the rows by hand would be
    /// testing `text_in` against rows nothing ever drew.
    fn paint_into(pinned: &Pinned, state: &State, width: u16, height: u16) {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        let state = state.clone();
        terminal
            .draw(|frame| state.paint(frame, Style::plain(), pinned))
            .expect("draw");
    }

    /// A frame painted into a fresh handle, for a test that wants both.
    fn painted_state(state: &State, width: u16, height: u16) -> std::sync::Arc<Pinned> {
        let pinned = Pinned::new();
        paint_into(&pinned, state, width, height);
        pinned
    }

    /// The whole interface painted once, as the rows the screen shows.
    fn screen_of(state: &State, width: u16, height: u16) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        let pinned = Pinned::new();
        let state = state.clone();
        terminal
            .draw(|frame| state.paint(frame, Style::plain(), &pinned))
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn the_sidebar_shows_the_connected_mcp_servers() {
        // The column is a column of state, and whether a server is on is part
        // of it: a person watching the work wants to see the servers their
        // tools came from beside it.
        let mut state = State::new();
        state.mcp = vec![
            McpRow {
                name: "context7".to_owned(),
                tools: 2,
                active: true,
            },
            McpRow {
                name: "everything".to_owned(),
                tools: 5,
                active: false,
            },
        ];
        let screen = screen_of(&state, 90, 24).join("\n");
        assert!(screen.contains("context7"), "{screen}");
        // A narrow column clips the name, which is why the row leads with its
        // state: the word is what has to survive the cut.
        assert!(screen.contains("everyth"), "{screen}");
        assert!(screen.contains("on  context7"), "{screen}");
        assert!(screen.contains("off everyth"), "{screen}");
        assert!(
            screen.contains("mcp 1/2"),
            "the count of what is on: {screen}"
        );
    }

    #[test]
    fn a_question_shows_its_prompt_and_every_choice() {
        // The bug this pins: the block counted a question as one row, so the
        // frame cut the list to the prompt and the choices never reached the
        // screen. Every choice has to be there, with the row that opens the box
        // under them.
        let mut state = State::new();
        state.question = Some(Question {
            prompt: "which one?".to_owned(),
            choices: vec!["alpha".to_owned(), "beta".to_owned()],
            cursor: 0,
            multiple: false,
            taken: Vec::new(),
            typing: false,
        });
        let screen = screen_of(&state, 60, 20).join("\n");
        assert!(screen.contains("which one?"), "{screen}");
        assert!(screen.contains("alpha"), "the first choice: {screen}");
        assert!(screen.contains("beta"), "the second choice: {screen}");
        assert!(screen.contains("something else"), "the box row: {screen}");
    }

    #[test]
    fn a_question_with_no_choices_still_shows_the_box_row() {
        // A model may ask without offering anything: the list is then the one row
        // that opens the box, and it has to be drawn or there is nothing to
        // choose at all.
        let mut state = State::new();
        state.question = Some(Question {
            prompt: "what now?".to_owned(),
            choices: Vec::new(),
            cursor: 0,
            multiple: false,
            taken: Vec::new(),
            typing: false,
        });
        let screen = screen_of(&state, 60, 20).join("\n");
        assert!(screen.contains("what now?"), "{screen}");
        assert!(screen.contains("something else"), "{screen}");
    }

    #[test]
    fn a_drag_copies_what_the_frame_painted_and_nothing_else() {
        // What the copy reads is the frame's own rows: the selection is in
        // cells of the screen, and only the frame knows where a line landed.
        // A blank row above the text is not part of a drag that starts on it.
        let mut state = State::new();
        state
            .transcript
            .push(Entry::line(Kind::Answer, "alpha beta gamma"));
        let pinned = painted_state(&state, 40, 6);

        // The answer is the last row, because the conversation sits against the
        // input. A drag across `beta` on it: columns 6..=9.
        //
        // Every drag repaints first: a copy takes the rows the frame recorded,
        // because what is on screen is what is being copied, and a second drag
        // without a frame would read the screen from before it.
        paint_into(&pinned, &state, 40, 6);
        let painted = pinned.debug_painted();
        // The row the answer landed on, found from the frame and not assumed:
        // the conversation sits against the input, so where it starts depends on
        // how much is in it and on the chrome below.
        let on_screen = painted
            .iter()
            .position(|line| line.contains("alpha beta gamma"))
            .expect("the answer on screen");
        let row = u16::try_from(on_screen).expect("a row of the screen");
        // The margin leads — a space and the bar — so the answer starts at the
        // column the frame reports and not at zero: `alpha` at 3, `beta` at 9.
        let line = &painted[on_screen];
        let start = u16::try_from(line.find("alpha").expect("the answer")).expect("a column");
        let word = u16::try_from(line.find("beta").expect("the word")).expect("a column");
        pinned.begin_selection(word, row);
        pinned.extend_selection(word + 3, row);
        assert_eq!(start, 3, "the margin is what the row leads with");
        let copied = pinned.finish_selection().expect("the selection");
        assert_eq!(copied, "beta", "the word that was under the drag");

        // A drag that starts on the blank rows above and ends on the text takes
        // the text and not the blanks it passed over.
        paint_into(&pinned, &state, 40, 6);
        let painted = pinned.debug_painted();
        let row = u16::try_from(
            painted
                .iter()
                .position(|line| line.contains("alpha beta gamma"))
                .expect("the answer on screen"),
        )
        .expect("a row of the screen");
        pinned.begin_selection(0, 0);
        pinned.extend_selection(4, row);
        let copied = pinned.finish_selection().expect("the selection");
        assert!(
            copied.starts_with("   al") && copied.ends_with("al"),
            "it starts and stops where the drag did: {copied:?}"
        );
        assert_eq!(
            copied.lines().count(),
            1,
            "the blank row above is not a line of the copy: {copied:?}"
        );

        // A drag over nothing is not a copy at all: an empty selection would
        // put a newline on the clipboard and lose whatever was there.
        paint_into(&pinned, &state, 40, 6);
        pinned.begin_selection(0, 0);
        pinned.extend_selection(3, 0);
        assert_eq!(
            pinned.finish_selection(),
            None,
            "a drag over nothing is not a copy"
        );
    }

    #[test]
    fn a_running_call_draws_one_row_that_turns_and_a_finished_one_keeps_its_place() {
        // One call is one row, and the frame is what shows it: while the call
        // runs the row leads with a turning glyph and the time it has taken,
        // and when it ends the same row — the same screen row — carries the
        // outcome. A test of the data alone cannot see either, because the glyph
        // and the clock are drawn and never stored.
        let mut state = State::new();
        let mut running = Entry::line(Kind::ToolStarted, "read README.md");
        running.started = Some(std::time::Instant::now());
        running.call_id = Some("call-1".to_owned());
        state.transcript.push(running);

        let rows = frame_of(&state, 60, 6);
        let live = rows
            .iter()
            .find(|row| row.contains("read README.md"))
            .expect("the live row");
        assert!(
            live.chars()
                .any(|glyph| SPINNER.contains(&glyph.to_string().as_str())),
            "a glyph of the spinner leads it: {live:?}"
        );
        assert!(live.contains("0s"), "and the clock runs: {live:?}");

        // The ending replaces the row's text, on the row it already had: the
        // glyph and the clock are the frame's, so a finished call has neither.
        let mut ended = Entry::line(Kind::ToolStarted, "read README.md");
        ended.kind = Kind::ToolOutput;
        ended.lines = vec![crate::transcript::Line::inked(
            crate::transcript::Ink::Green,
            "\u{2713} read in 3 ms",
        )];
        let mut done = State::new();
        done.transcript.push(ended);
        let rows = frame_of(&done, 60, 6);
        let finished = rows
            .iter()
            .find(|row| row.contains("read in 3 ms"))
            .expect("the finished row");
        assert!(
            !finished
                .chars()
                .any(|glyph| SPINNER.contains(&glyph.to_string().as_str())),
            "no glyph on a call that ended: {finished:?}"
        );
        assert!(!finished.contains("0s"), "and no clock: {finished:?}");
    }

    #[test]
    fn an_answer_is_drawn_as_markdown_and_a_notice_is_not() {
        // The data holds what arrived; the view is what knows it is markdown.
        // A tool's output is a command's bytes and a notice is a sentence, and
        // a `**` in either of those is two asterisks.
        let mut state = State::new();
        state.transcript.push(Entry::line(
            Kind::Answer,
            "## Title\n\n- one\n- two\n\nA **bold** word.",
        ));
        let rows = transcript_lines(&state.transcript, 60);
        let texts: Vec<&str> = rows.iter().map(|row| row.text.as_str()).collect();
        assert!(
            texts.iter().any(|line| line.contains("Title")),
            "the heading survives: {texts:?}"
        );
        assert!(
            !texts.iter().any(|line| line.contains('#')),
            "and its hashes do not: {texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|line| line.trim_start().starts_with("• one")),
            "a list is drawn with bullets: {texts:?}"
        );
        assert!(
            !texts.iter().any(|line| line.contains("**")),
            "and emphasis markers are gone: {texts:?}"
        );
        let bold = rows
            .iter()
            .flat_map(|row| row.runs.iter())
            .find(|run| run.text == "bold")
            .expect("the bold word is a run of its own");
        assert_eq!(bold.emphasis, crate::markdown::Emphasis::Bold);

        // The same text as a notice is left alone.
        let mut state = State::new();
        state
            .transcript
            .push(Entry::line(Kind::Notice, "## not a title and **not bold**"));
        let rows = transcript_lines(&state.transcript, 60);
        assert!(
            rows[0].text.contains("## not a title and **not bold**"),
            "a notice is text: {:?}",
            rows[0].text
        );
        assert!(rows[0].runs.is_empty(), "and it is one span");
    }

    #[test]
    fn a_fenced_block_is_drawn_as_code_on_every_row_it_spans() {
        // A fence is state that spans lines, which is why the answer is
        // rendered whole and not a line at a time.
        let mut state = State::new();
        state
            .transcript
            .push(Entry::line(Kind::Answer, "```rust\nlet x = 1;\n```"));
        let rows = transcript_lines(&state.transcript, 60);
        let code = rows
            .iter()
            .find(|row| row.text.contains("let x = 1;"))
            .expect("the code row");
        assert!(
            code.runs
                .iter()
                .any(|run| run.emphasis == crate::markdown::Emphasis::Fence),
            "drawn as code and not as prose: {:?}",
            code.runs
        );
        assert!(
            rows.iter().any(|row| row.text.contains("rust")),
            "the language leads the block: {:?}",
            rows.iter().map(|row| row.text.clone()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_conversation_paints_every_row_it_was_given() {
        // The area is fixed and the content is not. A short conversation left
        // the rows above it untouched, so whatever the previous frame had put
        // there stayed on screen: on a real terminal that was the tail of an
        // older answer interleaved with the new one.
        let mut state = State::new();
        state.transcript.push(Entry::line(Kind::Said, "hi"));
        let rows = frame_of(&state, 40, 6);
        assert_eq!(rows.len(), 6);
        // The line is the last row: the conversation sits against the input
        // and not against the top of an empty screen.
        assert!(
            rows[5].contains("hi"),
            "the newest line is at the bottom: {rows:?}"
        );
        // And every row above it was written as blank, not left as it was: an
        // untouched row keeps whatever the last frame put there.
        for row in &rows[..5] {
            assert!(
                row.trim().is_empty(),
                "an untouched row keeps old text: {row:?}"
            );
        }
    }

    #[test]
    fn the_view_shows_the_tail_until_someone_scrolls_back() {
        // The offset counts from the end, and the frame clamps it: the reader
        // moves a number of lines without knowing the height, so a scroll past
        // the top has to land on the first row and not on a negative one.
        assert_eq!(viewport(100, 20, 0), (80, 20), "the tail by default");
        assert_eq!(viewport(100, 20, 5), (75, 20), "five rows back");
        assert_eq!(
            viewport(100, 20, 95),
            (0, 20),
            "the top: a full screen, and the first row is the first row"
        );
        assert_eq!(
            viewport(100, 20, 10_000),
            (0, 20),
            "a wild scroll lands on the first row and not past it"
        );
        // A conversation shorter than the screen shows what there is, and the
        // rows it does not have are not blank ones.
        assert_eq!(viewport(3, 20, 0), (0, 3));
        assert_eq!(viewport(0, 20, 0), (0, 0), "nothing to draw");
    }

    #[test]
    fn the_margin_is_what_says_who_is_talking() {
        // Colour alone cannot carry it: on a terminal with no colour every ink
        // is the same ink, and the two sides of a conversation would read as
        // one voice. The bar is what survives that, which is why it is drawn
        // from the kind and not from the ink.
        let plain = Style::plain();
        assert_eq!(
            ink(Kind::Said, plain),
            ink(Kind::Answer, plain),
            "with no colour there is nothing to tell them apart"
        );
        assert_eq!(margin(Kind::Said).trim(), "▌");
        assert_eq!(margin(Kind::Reasoning).trim(), "│");
        assert_eq!(margin(Kind::Answer).trim(), "", "the model has no bar");
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
