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

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget as _};

use crate::style::{Style, icon, metrics};
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
    /// Whether the service connection is ready to accept input.
    pub(crate) input_ready: bool,
    /// Where the cursor is in the draft, in characters.
    pub(crate) cursor: usize,
    /// The other end of the draft's selection, when there is one.
    ///
    /// The cursor is one end and this is the other, so the frame draws the range
    /// the two describe as an inverted run — the same reverse video the mouse
    /// selection uses.
    pub(crate) select: Option<usize>,
    /// The completion, when the line offers one.
    pub(crate) completion: Option<Completion>,
    /// The question the model asked, when it is waiting for an answer.
    pub(crate) question: Option<Question>,
    /// The status row: the model, the dial, the cost, the effort.
    ///
    /// It is a styled line and not a string: the dial's colour says how close
    /// the compaction is and the effort bar's says which rung costs most, and a
    /// frame that stripped a painted string would lose both. The ink is chosen
    /// where those fields are built, and a frame draws it as it is.
    pub(crate) status: Line<'static>,
    /// The model's task list, as the compact footer widget shows it.
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
    /// What the mouse has selected, as two corners of the screen.
    ///
    /// The frame draws it and the frame is what reads it back as text: only it
    /// knows what was painted where, and the transcript behind it may have been
    /// wrapped differently for a terminal of another size.
    pub(crate) selection: Option<((u16, u16), (u16, u16))>,
}

/// One row of the task list, as the compact footer row draws it.
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

impl Completion {
    /// The first row the menu draws, so the cursor is on screen.
    ///
    /// The window is [`COMPLETION_ROWS`] rows tall and it follows the cursor
    /// rather than the head of the list: a cursor on the seventh row of a list
    /// of twenty drew the first four, so the arrows walked the list with nothing
    /// to show for it. The window stops at the last row that fits, which is what
    /// keeps the list from scrolling under itself at the end.
    #[must_use]
    fn top(&self) -> usize {
        let rows = self.rows.len();
        // The last row the window may start at, so it ends at the list's end.
        let last = rows.saturating_sub(COMPLETION_ROWS);
        self.cursor.min(last)
    }
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
    /// Whether the answer is a secret: no list, and only bullets are drawn.
    pub(crate) masked: bool,
}

impl State {
    /// An empty state, which is what a test draws from.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            input_ready: true,
            ..Self::default()
        }
    }

    /// The rows each band asks for at this width.
    ///
    /// This is the measure half of the contract: [`paint`](State::paint) hands
    /// every band exactly what it reports here, so a painter never has to guess
    /// the height it was given and the layout never has to guess what a painter
    /// will draw. The draft and the completion are separate bands, which is
    /// what keeps a tall draft from scrolling the menu off the screen.
    #[must_use]
    fn heights(&self, width: u16) -> crate::ui::Heights {
        let waiting = u16::from(self.waiting.is_some());
        let tasks = u16::from(!self.todos.is_empty());
        // A question takes its own rows: the prompt, every choice, the row that
        // opens the box, and the box itself once it is open. Counting it as one
        // row is what once cut the list to the prompt alone.
        let question = self.question.as_ref().map_or(0, |question| {
            // A secret has no list and no "something else" row: the prompt and
            // the box are the whole question.
            if question.masked {
                return 1_u16.saturating_add(u16::from(question.typing));
            }
            let choices = u16::try_from(question.choices.len()).unwrap_or(u16::MAX);
            // A free-text question opens its box directly, so the row that
            // would open it is not drawn and takes no room.
            let other = u16::from(!(question.typing && question.choices.is_empty()));
            1_u16
                .saturating_add(choices)
                .saturating_add(other)
                .saturating_add(u16::from(question.typing))
        });
        // A question holds the draft: the answer is typed into the question's
        // own box, so the draft band is empty while it is up.
        let input = if self.question.is_some() || !self.input_ready {
            0
        } else {
            self.draft_rows(width)
        };
        let completion = self.completion.as_ref().map_or(0, |completion| {
            u16::try_from(completion.rows.len().min(COMPLETION_ROWS)).unwrap_or(0)
        });
        crate::ui::Heights {
            spinner: waiting,
            tasks,
            question,
            input,
            completion,
            suggestion: 1,
            rule: 1,
            status: 1,
        }
    }

    /// The rows the draft wraps to at this width, clamped to [`INPUT_ROWS`].
    ///
    /// The clamp is a bound and not a reservation: a draft of one line leaves
    /// the rest to the conversation, and one taller than the bound scrolls to
    /// keep the cursor visible.
    #[must_use]
    fn draft_rows(&self, width: u16) -> u16 {
        let width = crate::app::draft_width(width);
        let rows = if self.input.is_empty() {
            1
        } else {
            self.input
                .split('\n')
                .map(|line| line.chars().count().div_ceil(width).max(1))
                .sum::<usize>()
        };
        u16::try_from(rows)
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
        modal: Option<&crate::modal::Modal>,
    ) -> Result<(), String> {
        terminal.draw(|frame| {
            self.paint(frame, style, pinned);
            // The panel is painted into the same frame as the interface. Two
            // `draw` calls are two flushes of the same terminal, and the screen
            // between them is the interface without its panel: that gap is the
            // flicker, and it is worst while the panel animates because every
            // frame is two writes instead of one.
            if let Some(modal) = modal {
                modal.draw(frame, frame.area(), style);
            }
        })
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
            //
            //   ┌──────────────────────────┐
            //   │ conversation   (scrolls) │
            //   ├──────────────────────────┤
            //   │ waiting                  │
            //   │ tasks                    │
            //   │ question                 │
            //   │ input                    │
            //   │ completion               │
            //   │ suggestion               │
            //   │ rule                     │
            //   │ status                   │
            //   └──────────────────────────┘
            //
            // Every band measures itself at this width and is handed exactly the
            // rows it asked for. That is the contract: the height a painter
            // assumes cannot drift from the rows it was given, because there is
            // only one number. The conversation takes whatever the chrome does
            // not, which is what keeps it scrollable against a terminal of any
            // height. The whole width is the conversation's: nothing is parked
            // beside it.
            let areas = crate::ui::split(area, self.heights(area.width));
            let widget = crate::ui::transcript::TranscriptView {
                state: self,
                pinned,
                style,
            };
            crate::ui::Widget::draw(&widget, frame, areas.conversation, &crate::style::Theme);
            if areas.spinner.height > 0 {
                let widget = crate::ui::spinner::Spinner {
                    waiting: self.waiting,
                    waiting_since: self.waiting_since,
                    style,
                };
                crate::ui::Widget::draw(&widget, frame, areas.spinner, &crate::style::Theme);
            }
            if areas.tasks.height > 0 {
                let widget = crate::ui::tasks::Tasks {
                    todos: &self.todos,
                    waiting: self.waiting.is_some(),
                    style,
                };
                crate::ui::Widget::draw(&widget, frame, areas.tasks, &crate::style::Theme);
            }
            if let Some(question) = self.question.as_ref().filter(|_| areas.question.height > 0) {
                draw_question(frame, areas.question, question, self, style);
            }
            if areas.input.height > 0 && self.input_ready {
                draw_input(frame, areas.input, self, style);
            }
            if areas.completion.height > 0
                && let Some(completion) = &self.completion
            {
                let top = completion.top();
                let widget = crate::ui::completion::CompletionMenu {
                    rows: &completion.rows,
                    cursor: completion.cursor,
                    top,
                    style,
                };
                crate::ui::Widget::draw(&widget, frame, areas.completion, &crate::style::Theme);
            }
            // The suggestion names what may be typed next, and when it names a
            // reasoning setting it means the same thing the bar in the status
            // does: it reads in the same colour, so the scale is learned once.
            if areas.suggestion.height > 0 {
                draw_suggestion(frame, areas.suggestion, &self.suggestion, style);
            }
            if areas.rule.height > 0 {
                draw_chrome_rule(frame, areas.rule, style);
            }
            // The status closes the frame: it is what the session *is* — the
            // model, the dial, the cost, the effort — and the bottom row is
            // where a status is read from.
            if areas.status.height > 0 {
                let room = usize::from(areas.status.width).saturating_sub(2).max(1);
                let mut line = self.status.clone();
                line.spans.insert(0, Span::raw("  "));
                let line = crate::ui::clip::clip_line(line, room);
                let widget = crate::ui::status::StatusRow { line, style };
                crate::ui::Widget::draw(&widget, frame, areas.status, &crate::style::Theme);
            }
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
pub(crate) const COMPLETION_ROWS: usize = metrics::COMPLETION_ROWS;

/// The most rows the input block may take, completion and question included.
pub(crate) const INPUT_ROWS: u16 = metrics::INPUT_ROWS;

/// Draws what the client is waiting for: a turning glyph and how long.
///
/// The glyph turns in both phases, because both are a wait: a wheel that stopped
/// when tokens started arriving would say the process had stalled at the exact
/// moment it was working. What changes with the phase is the words.
///
/// How long one glyph of the spinner is shown, in ms.
const SPINNER_MS: u64 = metrics::SPINNER_MS;

/// The frames of the spinner, in the order they turn.
const SPINNER: [&str; 4] = icon::SPINNER;

/// Draws a completion hint with the command name prominent and its arguments
/// quiet, so `/model` and the value it suggests are not one indistinguishable
/// colour.
fn draw_suggestion(frame: &mut Draw<'_>, area: Rect, text: &str, style: Style) {
    let (name, arguments) = text
        .split_once(' ')
        .map_or((text, ""), |(name, arguments)| (name, arguments));
    let line = Line::from(vec![
        Span::styled(name.to_owned(), style.accent_span()),
        Span::styled(
            if arguments.is_empty() {
                String::new()
            } else {
                format!(" {arguments}")
            },
            style.faint_span(),
        ),
    ]);
    Paragraph::new(line).render(area, frame.buffer_mut());
}

/// The bar in front of a row, which is what says who is talking.
///
/// It is a character and not a colour because a terminal with no colour has
/// none: the bar is the part that survives, and the ink on top of it says the
/// same thing twice for a terminal that can show it.
#[must_use]
pub(crate) fn margin(kind: Kind) -> String {
    match kind {
        Kind::Said => format!("{} ", icon::SAID),
        Kind::Reasoning => format!("{} ", icon::THINKING),
        _ => "  ".to_owned(),
    }
}

/// The columns the transcript's margin takes, with its leading space.
///
/// One space and the bar: the narrowest thing that still reads as a column of
/// its own beside the text.
pub(crate) const MARGIN_COLS: u16 = metrics::MARGIN_COLS;

/// Which part of a box a row is.
///
/// A tool's output is one bounded thing, and the frame is what says so. The
/// frame is drawn by the view and is not part of the row's text: a box whose
/// edges were rows of the conversation would be a box nobody can select the
/// inside of without taking the edges with them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Frame {
    /// A row between the rules.
    Inside,
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
    /// per emphasis — and the view is what turns an emphasis into a colour. A
    /// split diff row is one run per panel: the left in the ink of what left,
    /// the right in the ink of what arrived, which a single ink cannot say.
    pub(crate) runs: Vec<crate::markdown::Run>,
}

/// What the "back to the latest" bar says while the view is scrolled back.
///
/// It is the sign that what is on screen is not the end of the conversation,
/// and the way back to it: the row is a button the mouse can click, and the
/// key it names jumps there too. A reader who scrolled back should not have
/// to find the tail a page at a time.
const TAIL_BAR: &str = "↓ back to the latest · End";

/// Draws the conversation: the part that fits, newest at the bottom.
pub(crate) fn draw_transcript(
    frame: &mut Draw<'_>,
    area: Rect,
    state: &State,
    pinned: &crate::input::Pinned,
    style: Style,
) {
    if area.height == 0 {
        return;
    }
    // What the text has is what is left after the margin every row carries:
    // wrapping at the full width would put the last word of a row past the edge
    // of the terminal, where it is cut.
    let width = area.width.saturating_sub(MARGIN_COLS).max(1);
    let lines = transcript_lines(&state.transcript, width);
    // Every frame reads the scroll from the shared handle rather than the state
    // it was handed, and folds in the rows the conversation just gained. The
    // offset counts rows back from the end, so a chunk of a streaming answer
    // would otherwise push the text being read up the screen; counting the
    // growth into the offset holds it still. At the tail the offset is zero and
    // nothing is folded: there the newest line is the one to follow.
    pinned.transcript_grew(lines.len());
    let scroll = pinned.transcript_scroll();
    // From the bottom up: the newest line is the one being read, and a view
    // that grew from the top would put it off the screen. The frame is what
    // clamps the offset, because it is the only thing that knows how many rows
    // fit: the reader moves a number of lines and never knows the height.
    let (from, take) = viewport(lines.len(), area.height, scroll);
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
    // A bar over the bottom row while the view is scrolled back: it says
    // the end is not on screen and gives the one key, and the click, that
    // goes there. At the tail there is nothing to go back to, so no bar
    // is drawn and the frame records none: a click is a selection, as
    // everywhere else.
    if scroll == 0 || area.height == 0 {
        pinned.set_tail_bar(None);
        return;
    }
    let bar = Line::styled(TAIL_BAR.to_owned(), Style::reverse_span());
    let bar_row = area.y + area.height - 1;
    Paragraph::new(bar).render(
        Rect::new(area.x, bar_row, area.width, 1),
        frame.buffer_mut(),
    );
    pinned.set_tail_bar(Some((area.x, bar_row, area.width)));
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
            Ink::Alternate => style.alternate_span(),
            Ink::Green => style.green_span(),
            Ink::Red => style.red_span(),
            Ink::Yellow => style.yellow_span(),
        },
    );
    match row.frame {
        Some(Frame::Inside) => {
            // Tool output is a panel, not a collection of ASCII rules. The
            // background is the boundary; its content remains clean to copy.
            let room = usize::from(width);
            let content_width = room.saturating_sub(4);
            let padding = content_width.saturating_sub(row.text.chars().count());
            let mut spans = vec![Span::raw("  ".to_owned())];
            if row.runs.is_empty() {
                spans.push(Span::styled(
                    format!("{:<width$}", row.text, width = content_width),
                    ink,
                ));
            } else {
                spans.extend(row_spans(row, ink, style));
                spans.push(Span::styled(" ".repeat(padding), ink));
            }
            spans.push(Span::raw("  ".to_owned()));
            let mut line = Line::from(spans);
            line.style = style.output_background();
            line
        }
        None => {
            // The margin is what says who is talking. One column, before every
            // line: the person at `▌` and the model at a space, which is the
            // convention a chat and a diff already share. A colour alone cannot
            // carry it — a terminal with no colour shows none — and the margin
            // costs a column and reads at a glance.
            let bar_ink = match row.kind {
                Kind::Said => style.accent_span(),
                Kind::Reasoning => style.alternate_span(),
                _ => style.dim_span(),
            };
            let bar = margin(row.kind);
            let mut spans = vec![Span::raw(" "), Span::styled(bar, bar_ink)];
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
                crate::markdown::Emphasis::Link => draw = draw.fg(style.cyan_color()),
                // A strike is the line-through and nothing else: it is an
                // emphasis on the text, not a different kind of text.
                crate::markdown::Emphasis::Strike => {
                    draw = draw.add_modifier(Modifier::CROSSED_OUT);
                }
                // An underline keeps the ink and adds the line under it, the
                // way bold keeps the ink and adds weight.
                crate::markdown::Emphasis::Underline | crate::markdown::Emphasis::SyntaxString => {
                    draw = draw.add_modifier(Modifier::UNDERLINED);
                }
                // A table's header is its column names: bold, in the answer's
                // own ink, so it reads as a label and not as another colour.
                crate::markdown::Emphasis::TableHead
                | crate::markdown::Emphasis::SyntaxKeyword
                | crate::markdown::Emphasis::SyntaxNumber => {
                    draw = draw.add_modifier(Modifier::BOLD);
                }
                // A body cell is ordinary text: the padding and the `│` between
                // cells are what draw the table, and a colour of its own would
                // fight the prose around it. So it takes the same arm as plain
                // text, which is what it is.
                crate::markdown::Emphasis::TableCell
                | crate::markdown::Emphasis::Plain
                | crate::markdown::Emphasis::SyntaxPlain => {}
                // A panel of a split diff is its own ink, carried in the row
                // it came from: the left in the ink of what left, the right in
                // the ink of what arrived. The row's ink is the fallback for a
                // panel that carries none of its own.
                crate::markdown::Emphasis::Panel => {
                    if let Some(panel_ink) = run.ink {
                        draw = match panel_ink {
                            Ink::Faint => style.faint_span(),
                            Ink::Dim => style.dim_span(),
                            Ink::Alternate => style.alternate_span(),
                            Ink::Green => style.green_span(),
                            Ink::Red => style.red_span(),
                            Ink::Yellow => style.yellow_span(),
                        };
                    }
                }
                crate::markdown::Emphasis::SyntaxComment => {
                    draw = draw.add_modifier(Modifier::ITALIC);
                }
            }
            Span::styled(run.text.clone(), draw)
        })
        .collect()
}

/// Every entry of the conversation as the rows a terminal would show.
///
/// The wrap happens here and not in the data: a wrapped line is only correct at
/// the width it was wrapped for, and the width is the terminal's right now.
fn shell_command_runs(text: &str) -> Vec<crate::markdown::Run> {
    let Some((prefix, command)) = text.split_once("$ ") else {
        return Vec::new();
    };
    vec![
        crate::markdown::Run::plain(format!("{prefix}$ ")),
        crate::markdown::Run::styled(crate::markdown::Emphasis::Bold, command),
    ]
}

#[must_use]
pub(crate) fn transcript_lines(transcript: &Transcript, width: u16) -> Vec<Row> {
    let mut out = Vec::new();
    // The call the last entry reported on, so a padding row is not put inside a
    // call's own rows: a tool announcement and its output are one thing, and a
    // blank line between them would read as two.
    let mut last_call: Option<String> = None;
    for entry in transcript.entries() {
        push_padding(&mut out, entry, last_call.as_deref());
        last_call.clone_from(&entry.call_id);
        // A boxed entry opens with its rule, whatever it holds: the rule is what
        // says where the output begins, and an entry with no rows at all is one
        // the rule and the closing rule already describe.
        if entry.boxed
            && let Some(title) = &entry.title
        {
            let runs = shell_command_runs(title);
            out.push(Row {
                ink: Some(Ink::Alternate),
                kind: entry.kind,
                text: title.clone(),
                frame: Some(Frame::Inside),
                runs,
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
        let body: Vec<String> = entry
            .lines
            .iter()
            .map(crate::transcript::Line::text)
            .collect();
        let render_markdown = entry.kind == Kind::Answer
            || (entry.kind == Kind::ToolOutput && body.iter().any(|line| line.contains('`')));
        if render_markdown {
            for rendered in crate::markdown::render(&body.join("\n"), usize::from(width)) {
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
        let visible_lines = visible_output_lines(&mut out, entry);
        for (index, line) in visible_lines.iter().enumerate() {
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
                format!("{}  {}", line.text(), elapsed(at.elapsed()))
            } else {
                line.text()
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
                line.ink()
            };
            if line.parts.len() > 1 {
                out.push(panels_row(entry.kind, line, text, frame));
                continue;
            }
            let wrapped = wrap(&text, width);
            for text in wrapped {
                let runs = if line.runs.is_empty() {
                    shell_command_runs(&text)
                } else {
                    line.runs.clone()
                };
                out.push(Row {
                    ink,
                    kind: entry.kind,
                    text,
                    frame,
                    runs,
                });
            }
        }
        // What the bound dropped is said where it was dropped: a cut that hides
        // itself is worse than a short list that admits it.
        if entry.dropped > 0 && entry.collapsed_lines.is_none() {
            out.push(Row {
                ink: Some(Ink::Yellow),
                kind: Kind::Notice,
                text: format!("… {} earlier line(s) left out", entry.dropped),
                frame: entry.boxed.then_some(Frame::Inside),
                runs: Vec::new(),
            });
        }
    }
    out
}

/// One row of a split diff, as the view draws it: one run per panel.
///
/// Laid out from the panels and not from a joined string: the bound that
/// cut the row already cut each panel, and a wrap that re-joined them
/// would misplace every panel after the first. These rows are built at
/// the width the panels were laid out for, so the panels are the columns
/// the split view draws.
fn panels_row(
    kind: Kind,
    line: &crate::transcript::Line,
    text: String,
    frame: Option<Frame>,
) -> Row {
    let parts: Vec<(Option<Ink>, String)> = line
        .parts
        .iter()
        .map(|(ink, text)| (*ink, crate::select::strip_style(text).clone()))
        .collect();
    Row {
        ink: None,
        kind,
        text,
        frame,
        runs: parts
            .into_iter()
            .map(|(ink, text)| crate::markdown::Run::panel(ink.unwrap_or(Ink::Faint), text))
            .collect(),
    }
}

/// Adds the collapsed-output hint and returns the rows the view should draw.
fn visible_output_lines<'a>(
    out: &mut Vec<Row>,
    entry: &'a crate::transcript::Entry,
) -> &'a [crate::transcript::Line] {
    let Some(limit) = entry.collapsed_lines else {
        return &entry.lines;
    };
    let start = entry.lines.len().saturating_sub(limit);
    if !entry.expanded && start > 0 {
        out.push(Row {
            ink: Some(Ink::Yellow),
            kind: Kind::Notice,
            text: format!(
                "… {} line(s) hidden · Ctrl+O to expand",
                start.saturating_add(entry.dropped)
            ),
            frame: entry.boxed.then_some(Frame::Inside),
            runs: Vec::new(),
        });
    } else if entry.dropped > 0 {
        out.push(Row {
            ink: Some(Ink::Yellow),
            kind: Kind::Notice,
            text: format!(
                "… {} earlier line(s) omitted (retention limit)",
                entry.dropped
            ),
            frame: entry.boxed.then_some(Frame::Inside),
            runs: Vec::new(),
        });
    }
    if entry.expanded {
        &entry.lines
    } else {
        &entry.lines[start..]
    }
}

/// Puts a blank row between two entries that are not the same call.
///
/// Padding goes between entries and not between their rows: a message against
/// the one above it reads as a continuation of it, and the whole conversation as
/// one wall. A tool announcement and its output are one call, so they stay
/// together. The blank row carries no frame and no kind of its own, so a
/// selection over it copies a blank line and not part of a box.
fn push_padding(out: &mut Vec<Row>, entry: &crate::transcript::Entry, last_call: Option<&str>) {
    if out.is_empty() || entry.is_empty() {
        return;
    }
    let same_call = entry
        .call_id
        .as_deref()
        .is_some_and(|id| Some(id) == last_call);
    if same_call {
        return;
    }
    out.push(Row {
        ink: None,
        kind: Kind::Notice,
        text: String::new(),
        frame: None,
        runs: Vec::new(),
    });
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
                        ink: run.ink,
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
                ink: run.ink,
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
        Kind::Said => style.cyan_span(),
        Kind::Answer => ratatui::style::Style::new(),
        Kind::Reasoning | Kind::ToolOutput => style.alternate_span(),
        Kind::Notice => style.dim_span(),
        Kind::ToolStarted => style.accent_span(),
        Kind::Failure => style.red_span(),
    }
}

/// Draws a horizontal rule across the top edge of an area.
fn draw_chrome_rule(frame: &mut Draw<'_>, area: Rect, style: Style) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let line = Line::from(Span::styled(
        "─".repeat(usize::from(area.width)),
        style.ratatui(crate::style::RULE, 238, "90"),
    ));
    let strip = Rect::new(area.x, area.y, area.width, 1);
    Paragraph::new(line).render(strip, frame.buffer_mut());
}

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
        style.yellow_span(),
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
    // answerable, and this is the row that makes it so. A secret has no list to
    // open a box from: the box is the question.
    if !(question.masked || question.typing && question.choices.is_empty()) {
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

/// The columns a draft line wraps to, after the prompt takes its own.
///
/// Measure and paint both call this: the wrap the height is counted from and
/// the wrap the rows are drawn in are the same number, so a full line cannot
/// be counted as one row and drawn as two.
#[must_use]
fn draft_width(area_width: u16) -> usize {
    let prompt = icon::PROMPT.chars().count() + 1;
    usize::from(area_width).saturating_sub(prompt).max(1)
}

/// Draws the input block: the draft and its cursor.
///
/// The question and the completion used to live in these rows too, which meant
/// three things competed for one band and the height of each was guessed from
/// the other two. They are bands of their own now; this painter draws exactly
/// the draft, and [`State::heights`] gave it exactly the rows the wrap needs.
#[allow(clippy::too_many_lines)]
fn draw_input(frame: &mut Draw<'_>, area: Rect, state: &State, style: Style) {
    if area.height == 0 {
        return;
    }
    let mut rows: Vec<Line> = Vec::new();
    // The draft, wrapped to the width it has and with the cursor drawn as the
    // character it is on. Wrapping is what makes a long line readable: a draft
    // the terminal cut off at the edge would hide the words being typed, which
    // is the one thing the person is looking at.
    let prompt = format!("{} ", icon::PROMPT);
    let width = draft_width(area.width);
    let mut cursor_row = 0usize;
    // The selected range, in the draft's own character indices, normalised so
    // the low end is first: a selection drawn backwards is the same text as one
    // drawn forwards, and only the ranges between the ends are inverted.
    let selection = state.select.map(|anchor| {
        if anchor <= state.cursor {
            (anchor, state.cursor)
        } else {
            (state.cursor, anchor)
        }
    });
    let lines: Vec<&str> = state.input.split('\n').collect();
    let mut offset = 0usize;
    for (index, line) in lines.iter().enumerate() {
        let len = line.chars().count();
        // Whether the cursor is on *this* logical line at all. A line that does
        // not hold it has no cursor of its own: reading the clamp below as "the
        // cursor is at the end of every line" is what drew a box on each one.
        let holds_cursor = offset <= state.cursor && state.cursor <= offset + len;
        let at = state.cursor.saturating_sub(offset).min(len);
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
            if holds_cursor
                && (from <= at && at < from + part.len()
                    || at == len && from + part.len() == chars.len())
            {
                cursor_row = rows.len();
            }
            let label = if index == 0 && index_of_row == 0 {
                prompt.as_str()
            } else {
                "  "
            };
            let label_ink = if index == 0 && index_of_row == 0 {
                style.accent_span()
            } else {
                style.faint_span()
            };
            let mut spans = vec![Span::styled(label, label_ink)];
            for (position, character) in part.iter().enumerate() {
                // The absolute index of this character in the draft, which is
                // what both the cursor and the selection are measured in.
                let at_char = offset + from + position;
                // The cursor is one cell and the selection is a run of them; both
                // are drawn the same way, so one test decides for either. The
                // cursor is the cell the next keystroke lands on, and it is part
                // of the run whenever the selection reaches it.
                let selected = selection.is_some_and(|(lo, hi)| lo <= at_char && at_char < hi);
                let cursor = holds_cursor && from + position == at;
                if cursor || selected {
                    spans.push(Span::styled(character.to_string(), Style::selected()));
                } else {
                    spans.push(Span::raw(character.to_string()));
                }
            }
            // A cursor at the very end of the line still needs a cell to sit
            // on, or it would be invisible at the one moment it is looked for.
            if holds_cursor && at == len && from + part.len() == chars.len() {
                spans.push(Span::styled(" ", Style::selected()));
            }
            rows.push(Line::from(spans));
        }
        offset += len + 1;
    }
    // A draft taller than the block shows the rows around the cursor rather
    // than the first rows: the line being written is the one being looked at,
    // and a block pinned to its top would scroll the cursor off the bottom.
    let height = usize::from(area.height);
    let start = if cursor_row >= height {
        cursor_row + 1 - height
    } else {
        0
    };
    draw_block_from(frame, area, &rows, start);
}

/// Draws rows inside a block, from the top of the area it was given.
fn draw_block(frame: &mut Draw<'_>, area: Rect, rows: &[Line]) {
    draw_block_from(frame, area, rows, 0);
}

/// Draws rows inside a block, starting at `start`, clipped to the area.
///
/// The offset is what keeps a draft taller than the block around its cursor:
/// a block that always started at row zero would scroll the line being typed
/// off its own bottom whenever the draft grew past the block.
fn draw_block_from(frame: &mut Draw<'_>, area: Rect, rows: &[Line], start: usize) {
    let from = start.min(rows.len());
    let take = usize::from(area.height).min(rows.len() - from);
    let inside = Rect::new(
        area.x,
        area.y,
        area.width,
        u16::try_from(take).unwrap_or(area.height),
    );
    Paragraph::new(rows[from..from + take].to_vec()).render(inside, frame.buffer_mut());
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
        // The two are a said line and an answer, in that order, with the padding
        // row between them: the order is what this pins, not the row count.
        let said = lines
            .iter()
            .position(|row| row.text == "hello")
            .expect("the said line");
        let answer = lines
            .iter()
            .position(|row| row.text == "hi")
            .expect("the answer");
        assert_eq!(lines[said].kind, Kind::Said);
        assert_eq!(lines[answer].kind, Kind::Answer);
        assert!(said < answer, "the order is the order it happened in");
    }

    #[test]
    fn a_blank_row_separates_two_entries_and_not_a_call() {
        // Two messages are two things and a blank row says so; a tool
        // announcement and its output are one call and stay together.
        let mut transcript = Transcript::new();
        transcript.push(Entry::line(Kind::Said, "hello"));
        transcript.push(Entry::line(Kind::Answer, "hi"));
        let rows = transcript_lines(&transcript, 80);
        let said = rows
            .iter()
            .position(|row| row.text == "hello")
            .expect("said");
        let answer = rows
            .iter()
            .position(|row| row.text == "hi")
            .expect("answer");
        assert_eq!(answer - said, 2, "a blank row between them: {rows:?}");
        assert!(rows[said + 1].text.is_empty(), "{rows:?}");

        // One call: the announcement and the output share a call id.
        let mut transcript = Transcript::new();
        let mut started = Entry::line(Kind::ToolStarted, "→ shell");
        started.call_id = Some("c1".to_owned());
        let mut output = Entry::line(Kind::ToolOutput, "done");
        output.call_id = Some("c1".to_owned());
        transcript.push(started);
        transcript.push(output);
        let rows = transcript_lines(&transcript, 80);
        let first = rows
            .iter()
            .position(|row| row.text.contains("shell"))
            .expect("shell");
        let second = rows
            .iter()
            .position(|row| row.text == "done")
            .expect("done");
        assert_eq!(second - first, 1, "no blank row inside a call: {rows:?}");
    }

    #[test]
    fn a_boxed_entry_uses_a_background_without_ascii_rules() {
        let mut transcript = Transcript::new();
        let mut entry = Entry::bordered(Kind::ToolOutput, "shell #abc");
        entry.push_line("one");
        entry.push_line("two");
        transcript.push(entry);
        let rows = transcript_lines(&transcript, 80);
        assert_eq!(rows[0].frame, Some(Frame::Inside));
        assert_eq!(rows[0].text, "shell #abc");
        assert_eq!(rows[1].frame, Some(Frame::Inside));
        assert_eq!(rows[1].text, "one");
        assert_eq!(rows[2].frame, Some(Frame::Inside));
        assert_eq!(rows[2].text, "two");
        for row in &rows[..] {
            assert!(
                !row.text
                    .chars()
                    .any(|character| matches!(character, '│' | '─' | '╭' | '╮' | '╰' | '╯')),
                "the content carries no ASCII-style frame: {:?}",
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
        assert_eq!(state.heights(80).input, 1, "an empty draft is one row");
        state.input = "one\ntwo\nthree".to_owned();
        assert_eq!(state.heights(80).input, 3);
        // And it stops: a draft of a thousand lines is not a reason to lose the
        // conversation.
        state.input = "x\n".repeat(1_000);
        assert_eq!(state.heights(80).input, INPUT_ROWS);
    }

    #[test]
    fn the_completion_is_its_own_band_so_a_tall_draft_cannot_hide_it() {
        let mut state = State::new();
        state.input = "x\n".repeat(100);
        state.completion = Some(Completion {
            text: "alpha".to_owned(),
            rows: vec!["alpha".to_owned(), "beta".to_owned()],
            cursor: 0,
        });
        let heights = state.heights(80);
        assert_eq!(heights.input, INPUT_ROWS, "the draft keeps its own rows");
        assert_eq!(heights.completion, 2, "the menu keeps its own rows");
        assert!(
            heights.input + heights.completion
                <= INPUT_ROWS + u16::try_from(COMPLETION_ROWS).unwrap_or(0),
            "the two are measured separately, not out of one budget"
        );
    }

    #[test]
    fn the_input_wraps_at_the_body_width_and_shows_every_row() {
        // The conversation owns the whole width now, so the draft wraps at the
        // terminal's width. The block's height is measured there too: counting
        // it narrower than the text gave it one row fewer than it needed and cut
        // the last row off — the words being typed.
        let mut state = State::new();
        state.input = "x".repeat(70);
        let rows_of_x: Vec<usize> = screen_of(&state, 80, 24)
            .iter()
            .map(|row| row.chars().filter(|c| *c == 'x').count())
            .filter(|count| *count > 0)
            .collect();
        assert_eq!(
            rows_of_x.len(),
            1,
            "the draft fits one row at the full width: {rows_of_x:?}"
        );
        assert_eq!(
            rows_of_x.iter().sum::<usize>(),
            70,
            "and no character was lost: {rows_of_x:?}"
        );
    }

    /// A completion menu with `rows` rows and its cursor at `cursor`.
    fn menu(rows: usize, cursor: usize) -> Completion {
        Completion {
            text: String::new(),
            rows: (0..rows).map(|n| format!("model-{n}")).collect(),
            cursor,
        }
    }

    #[test]
    fn the_completion_menu_follows_its_cursor_instead_of_its_head() {
        // The menu drew the first four rows whatever the cursor did, so a list
        // of ten walked from the fifth row on showed nothing: the arrows moved a
        // choice that was below the window the whole way. The window follows the
        // cursor now, and the row under it is the row Enter would take.
        let mut state = State::new();
        state.completion = Some(menu(10, 7));
        let screen = screen_of(&state, 80, 24).join("\n");
        assert!(
            screen.contains("model-7"),
            "the row under the cursor is on screen: {screen}"
        );
        assert!(
            !screen.contains("model-0"),
            "and the head of the list is not: the window moved: {screen}"
        );

        // It stops at the end rather than scrolling past it: the last row is on
        // screen and the window is full, so the list never shows blank rows.
        state.completion = Some(menu(10, 9));
        let end = screen_of(&state, 80, 24).join("\n");
        assert!(end.contains("model-9"), "the last row is shown: {end}");
        assert!(end.contains("model-6"), "and the window is full: {end}");
        assert!(
            !end.contains("model-5"),
            "starting at the list's end: {end}"
        );
    }

    #[test]
    fn a_menu_shorter_than_the_window_starts_at_its_first_row() {
        // The window is `COMPLETION_ROWS` tall and a list of three fills three
        // rows from the top: there is nowhere to scroll and the arithmetic that
        // moves it must not cut the head off a list that fits.
        let mut state = State::new();
        state.completion = Some(menu(3, 2));
        let screen = screen_of(&state, 80, 24).join("\n");
        for row in ["model-0", "model-1", "model-2"] {
            assert!(screen.contains(row), "`{row}` is shown: {screen}");
        }
    }

    #[test]
    fn a_draft_taller_than_the_block_shows_the_line_the_cursor_is_on() {
        // The block is bounded, and a draft past the bound scrolls inside it. A
        // block pinned to its first row scrolls the cursor off the bottom, and
        // the line being written is the one thing the block exists to show.
        let mut state = State::new();
        state.input = (0..30)
            .map(|n| format!("row {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        state.cursor = state.input.chars().count();
        let screen = screen_of(&state, 80, 24).join("\n");
        assert!(
            screen.contains("row 29"),
            "the last row, where the cursor is, is shown: {screen}"
        );
    }

    #[test]
    fn the_selection_is_drawn_as_an_inverted_run() {
        // A selection in the draft is the same reverse video the mouse selection
        // uses: the frame inverts the range and the cursor is one cell of it.
        let mut state = State::new();
        state.input = "hello world".to_owned();
        state.cursor = 5;
        state.select = Some(0);
        let reversed = reversed_rows(&state, 40, 12);
        let row = reversed
            .iter()
            .position(|count| *count > 0)
            .expect("the draft's row");
        // Five selected characters and the cursor's own cell on the space after
        // `hello`: the run is the selection, and the cursor sits at its end.
        assert!(
            reversed[row] >= 5,
            "the selected run is inverted: {reversed:?}"
        );

        // With no selection only the cursor is inverted, which is one cell.
        let mut plain = State::new();
        plain.input = "hello world".to_owned();
        plain.cursor = 5;
        let reversed = reversed_rows(&plain, 40, 12);
        let row = reversed
            .iter()
            .position(|count| *count > 0)
            .expect("the draft's row");
        assert_eq!(
            reversed[row], 1,
            "with no selection one cell is the cursor: {reversed:?}"
        );
    }

    #[test]
    fn the_cursor_is_a_box_on_one_row_and_not_on_every_line() {
        // Reading the clamp as "the cursor sits at the end of every line" drew a
        // box after each one. There is one cursor, and it sits on one row.
        let mut state = State::new();
        state.input = "alpha\nbeta\ngamma".to_owned();
        state.cursor = state.input.chars().count();
        // The cursor cells are the reversed ones: count them per screen row.
        let reversed = reversed_rows(&state, 40, 12);
        let with_cursor: Vec<usize> = reversed
            .iter()
            .enumerate()
            .filter(|(_, count)| **count > 0)
            .map(|(row, _)| row)
            .collect();
        assert_eq!(
            with_cursor.len(),
            1,
            "there is one cursor row, not one per line: {reversed:?}"
        );
        // And the two lines above it carry no cursor at all.
        assert!(
            with_cursor[0] >= 2,
            "the cursor is on the last line, not the first: {with_cursor:?}"
        );
    }

    #[test]
    fn a_line_at_the_end_does_not_move_what_is_being_read() {
        // While an answer streams in, a person reading history stays on the line
        // they were on. The offset counts rows back from the end, so a line
        // arriving at the end used to shift the window up by a row and the text
        // under the eyes walked away; the frame folds that growth into the offset
        // now, which is what holds the view still.
        let mut transcript = Transcript::new();
        for index in 0..40 {
            transcript.push(Entry::line(Kind::Answer, format!("line {index}")));
        }
        let width = 80;
        let pinned = Pinned::new();
        // Three rows back from the tail, which is where a scroll leaves it.
        pinned.scroll_transcript(-3);

        let before = transcript_lines(&transcript, width);
        pinned.transcript_grew(before.len());
        let (from, take) = viewport(before.len(), 20, pinned.transcript_scroll());
        let seen: Vec<&str> = before[from..from + take]
            .iter()
            .map(|row| row.text.as_str())
            .collect();

        // A chunk of the streaming answer lands as two more rows.
        transcript.push(Entry::line(Kind::Answer, "one more".to_owned()));
        let after = transcript_lines(&transcript, width);
        let added = after.len().saturating_sub(before.len());
        assert!(added >= 1, "the new entry added rows: {added}");
        pinned.transcript_grew(after.len());
        let (from, take) = viewport(after.len(), 20, pinned.transcript_scroll());
        let now: Vec<&str> = after[from..from + take]
            .iter()
            .map(|row| row.text.as_str())
            .collect();

        assert_eq!(
            now, seen,
            "the rows under the eye did not move as the answer grew"
        );
    }

    #[test]
    fn the_view_follows_the_tail_while_it_is_at_the_tail() {
        // At the tail the offset is zero and nothing is folded: the newest line
        // is the one to show, so a chunk arriving has to push the window forward
        // with it.
        let mut transcript = Transcript::new();
        for index in 0..40 {
            transcript.push(Entry::line(Kind::Answer, format!("line {index}")));
        }
        let width = 80;
        let pinned = Pinned::new();
        let before = transcript_lines(&transcript, width);
        pinned.transcript_grew(before.len());
        transcript.push(Entry::line(Kind::Answer, "one more".to_owned()));
        let after = transcript_lines(&transcript, width);
        pinned.transcript_grew(after.len());
        assert_eq!(pinned.transcript_scroll(), 0, "the tail stays the tail");
        let (from, take) = viewport(after.len(), 20, pinned.transcript_scroll());
        assert_eq!(from + take, after.len(), "the last row is on screen");
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
                draw_transcript(frame, body, &state, &Pinned::new(), Style::plain());
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
                    draw_transcript(frame, area, &state, &Pinned::new(), Style::plain());
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

    #[test]
    fn a_scrolled_view_shows_a_way_back_and_the_tail_does_not() {
        // Reading history with no sign that it is history is how a person loses
        // the newest line: the bar is the sign and the button, and it is gone at
        // the tail, where there is nothing to go back to.
        let mut state = State::new();
        for index in 0..40 {
            state
                .transcript
                .push(Entry::line(Kind::Answer, format!("line {index}")));
        }
        // At the tail there is no bar.
        let at_tail = painted_state(&state, 60, 8);
        assert!(at_tail.tail_bar().is_none(), "no bar is drawn at the tail");
        // Scrolled back, the bar is drawn on the bottom row and is clickable.
        let scrolled = Pinned::new();
        scrolled.scroll_transcript(-5);
        paint_into(&scrolled, &state, 60, 8);
        let (left, row, width) = scrolled.tail_bar().expect("a bar when scrolled");
        assert!(width > 0 && left < 60, "the bar spans into the frame");
        assert!(row < 8, "the bar is inside an 8-row frame: {row}");
        assert!(
            scrolled.click_tail_bar(left, row),
            "a click on the bar is caught"
        );
        assert_eq!(scrolled.transcript_scroll(), 0, "and goes to the tail");
    }

    #[test]
    fn a_click_away_from_the_bar_does_not_jump() {
        // The bar is a button, not a hot zone over the conversation: a click on
        // the text is a selection, which is what reading and copying need.
        let pinned = Pinned::new();
        pinned.scroll_transcript(-5);
        assert!(
            !pinned.click_tail_bar(0, 0),
            "nothing is drawn there, so nothing was caught"
        );
        assert_eq!(pinned.transcript_scroll(), 5, "the view did not move");
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

    /// The whole interface painted once, as the reversed (cursor) cells per row.
    ///
    /// It is what tells a real cursor from a stray one: the text alone cannot
    /// say which cells were inverted, and the cursor is the inversion.
    fn reversed_rows(state: &State, width: u16, height: u16) -> Vec<usize> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::style::Modifier;
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
                    .filter(|x| buffer[(*x, y)].modifier.contains(Modifier::REVERSED))
                    .count()
            })
            .collect()
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
            masked: false,
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
            masked: false,
        });
        let screen = screen_of(&state, 60, 20).join("\n");
        assert!(screen.contains("what now?"), "{screen}");
        assert!(screen.contains("something else"), "{screen}");
    }

    #[test]
    fn a_masked_question_is_a_prompt_and_a_box_without_a_list() {
        let mut state = State::new();
        state.question = Some(Question {
            prompt: "API key for ollamacloud".to_owned(),
            choices: Vec::new(),
            cursor: 0,
            multiple: false,
            taken: Vec::new(),
            typing: true,
            masked: true,
        });
        state.input = "••••••".to_owned();
        let screen = screen_of(&state, 60, 20).join("\n");
        assert!(screen.contains("API key for ollamacloud"), "{screen}");
        assert!(screen.contains("••••••"), "the masked box: {screen}");
        assert!(!screen.contains("something else"), "no list: {screen}");
    }

    #[test]
    fn a_free_text_question_opens_its_box_without_the_other_row() {
        let mut state = State::new();
        state.question = Some(Question {
            prompt: "what now?".to_owned(),
            choices: Vec::new(),
            cursor: 0,
            multiple: false,
            taken: Vec::new(),
            typing: true,
            masked: false,
        });
        state.input = "the answer".to_owned();
        let screen = screen_of(&state, 60, 20).join("\n");
        assert!(screen.contains("what now?"), "{screen}");
        assert!(screen.contains("the answer"), "the box: {screen}");
        assert!(
            !screen.contains("something else"),
            "nothing to choose: {screen}"
        );
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
        assert_eq!(copied, "al", "the drag's own text, without the margin");
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
