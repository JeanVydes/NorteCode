//! The modal: a panel of text over the interface, and what a key means in it.
//!
//! A command that reports something — `/tools`, `/debug`, `/jobs` — opens a
//! panel. Some of those reports are also *editable*: the tools list is where a
//! person allows or denies a tool, and the MCP list is where a server is
//! switched off. Making them read-only meant typing `/allow shell global` from
//! memory while looking at the list that names the tool, which is a command
//! line doing a panel's job.
//!
//! So a panel has a cursor, and a key is a [`Choice`]: a decision the panel
//! makes and the caller carries out. The panel does not know what allowing a
//! tool does — it cannot, the service is not here — so what it produces is the
//! intent, and the loop that owns the service is what acts on it.
//!
//! # What it is not
//!
//! It is not a widget library. A row is a line of text, an [`Action`] is a
//! decision, and one place draws them. A plugin that wants to draw its own
//! panel needs the state and the area, and that is what this is.
//!
//! # The scope
//!
//! An edit has a scope: a session, or every session. It is a key rather than a
//! second panel, because the choice is part of the decision and not something
//! to be asked again for every row — a person who is denying ten tools is
//! denying them for the same scope.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Widget as _};

use crate::style::Style;
use crate::terminal::Draw;

/// One row of a panel.
///
/// A row is what it says and, when it can be acted on, what acting on it means.
/// A report is a list of rows that carry nothing; an editable panel is a list
/// of rows that carry an [`Action`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Row {
    /// What the row says.
    pub text: String,
    /// Whether it is a heading, which is drawn and never walked.
    pub heading: bool,
    /// What acting on this row means, when it can be acted on.
    pub action: Option<Action>,
    /// A word on the right that says what the row's state is.
    pub note: Option<String>,
    /// Whether the row is a quiet note, drawn dim.
    ///
    /// A telemetry panel is mostly bars and numbers, and the lines around them —
    /// a hint, a subtitle — are read past. The flag is on the row and not the
    /// text so a dim row is still text a person can select and copy.
    pub dim: bool,
}

impl Row {
    /// A row that says something and nothing more.
    #[must_use]
    pub(crate) fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            heading: false,
            action: None,
            note: None,
            dim: false,
        }
    }

    /// A heading, which the cursor walks past.
    #[must_use]
    pub(crate) fn heading(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            heading: true,
            action: None,
            note: None,
            dim: false,
        }
    }

    /// A row that can be acted on.
    #[must_use]
    pub(crate) fn actable(text: impl Into<String>, action: Action) -> Self {
        Self {
            text: text.into(),
            heading: false,
            action: Some(action),
            note: None,
            dim: false,
        }
    }

    /// The same row with a word on the right.
    #[must_use]
    pub(crate) fn noted(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }
}

/// What a key asked the panel to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Choice {
    /// Nothing was decided.
    None,
    /// The panel was closed.
    Closed,
    /// Act on the row under the cursor.
    Act {
        /// What the row carries.
        action: Action,
        /// Whether it applies to every session or only to this one.
        scope: Scope,
    },
}

/// What acting on a row means.
///
/// It is an intent and not a call: the panel has no service and cannot say
/// whether a tool exists or a server is connected. The loop that owns the
/// service reads this and answers, and the panel is redrawn from what it is
/// given next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    /// Allow a tool without asking again.
    Allow(String),
    /// Deny a tool.
    Deny(String),
    /// Forget a tool's rule, so a call to it asks again.
    Forget(String),
    /// Switch an MCP server on or off.
    Toggle(String),
}

/// How far a rule reaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum Scope {
    /// Only this session.
    #[default]
    Session,
    /// Every session.
    Global,
}

impl Scope {
    /// The other scope, which is what the key toggles to.
    #[must_use]
    fn flipped(self) -> Self {
        match self {
            Self::Session => Self::Global,
            Self::Global => Self::Session,
        }
    }

    /// The word the panel shows for it.
    #[must_use]
    fn word(self) -> &'static str {
        match self {
            Self::Session => "this session",
            Self::Global => "global",
        }
    }
}

/// What a key means in a panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Key {
    /// Move the cursor up.
    Up,
    /// Move the cursor down.
    Down,
    /// Move the cursor a page down.
    PageDown,
    /// Move the cursor a page up.
    PageUp,
    /// Act on the row under the cursor.
    Enter,
    /// Flip the scope of the next action.
    ToggleScope,
    /// Switch to the next tab.
    Tab,
    /// Walk the range selector on.
    NextRange,
    /// Walk the range selector back.
    PrevRange,
    /// Walk the metric the ranks are ordered by.
    NextMetric,
    /// Walk the metric back.
    PrevMetric,
    /// Leave.
    Escape,
}

/// A panel of rows over the interface.
///
/// Not `PartialEq`: a telemetry panel carries sums of money, and comparing two
/// of those is not something the client does or should.
#[derive(Clone, Debug)]
pub(crate) struct Modal {
    /// What the panel is about, on its top border.
    pub(crate) title: String,
    /// The rows, in the order they are drawn.
    pub(crate) rows: Vec<Row>,
    /// Which row the cursor is on, as an index into `rows`.
    pub(crate) cursor: usize,
    /// Which row is at the top of the panel.
    ///
    /// It counts from the beginning, which is what a report is read from: the
    /// first row is the first thing it says.
    pub(crate) top: usize,
    /// How far the next action reaches.
    pub(crate) scope: Scope,
    /// Whether the panel can be acted on at all.
    ///
    /// A report and an editor are the same panel: a report has no actable rows
    /// and the cursor skips it, which is what keeps one widget for both.
    pub(crate) editable: bool,
    /// The telemetry panel, when this panel is one.
    ///
    /// It carries its own state — the tab, the range, the metric, what is
    /// expanded — because a chart panel is not a list of rows: the rows are
    /// rebuilt from that state on every frame, and the keys move the state and
    /// not a cursor over text.
    pub(crate) telemetry: Option<crate::telemetry_panel::Panel>,
}

impl Modal {
    /// A report: rows to read, no cursor, no keys but closing.
    #[must_use]
    pub(crate) fn report(title: impl Into<String>, lines: Vec<String>) -> Self {
        Self {
            title: title.into(),
            rows: lines.into_iter().map(Row::text).collect(),
            cursor: 0,
            top: 0,
            scope: Scope::Session,
            editable: false,
            telemetry: None,
        }
    }

    /// An editor: rows with a cursor, and keys that act on them.
    #[must_use]
    pub(crate) fn editable(title: impl Into<String>, rows: Vec<Row>) -> Self {
        let mut modal = Self {
            title: title.into(),
            rows,
            cursor: 0,
            top: 0,
            scope: Scope::Session,
            editable: true,
            telemetry: None,
        };
        modal.cursor = modal.first_actable();
        modal
    }

    /// A panel that draws the telemetry charts and walks its own tabs.
    #[must_use]
    pub(crate) fn telemetry(panel: crate::telemetry_panel::Panel) -> Self {
        Self {
            title: "telemetry".to_owned(),
            rows: Vec::new(),
            cursor: 0,
            top: 0,
            scope: Scope::Session,
            editable: false,
            telemetry: Some(panel),
        }
    }

    /// Rebuilds the rows of a telemetry panel from its state.
    ///
    /// The panel draws itself: it is not a list the frame scrolls, it is a chart
    /// the frame paints, so its lines are made here where the width is known and
    /// the frame only draws them.
    pub(crate) fn refresh_telemetry(&mut self) {
        let Some(panel) = self.telemetry.as_ref() else {
            return;
        };
        self.rows = panel
            .lines()
            .into_iter()
            .map(|line| {
                if line.bold {
                    Row::heading(line.text)
                } else if line.dim {
                    let mut row = Row::text(line.text);
                    row.dim = true;
                    row
                } else {
                    Row::text(line.text)
                }
            })
            .collect();
    }

    /// The first row that can be acted on, or zero when none can.
    #[must_use]
    fn first_actable(&self) -> usize {
        self.rows
            .iter()
            .position(|row| row.action.is_some())
            .unwrap_or(0)
    }

    /// Whether the panel is animating, so the clock has to keep drawing it.
    #[must_use]
    pub(crate) fn animating(&self) -> bool {
        self.telemetry
            .as_ref()
            .is_some_and(crate::telemetry_panel::Panel::animating)
    }

    /// Advances the panel's animation one frame and rebuilds the rows.
    pub(crate) fn tick(&mut self) {
        let Some(panel) = self.telemetry.as_mut() else {
            return;
        };
        panel.tick();
        self.refresh_telemetry();
    }

    /// Whether there is anything to draw.
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Moves the cursor by rows, holding at both ends and skipping headings.
    ///
    /// A heading is drawn and never landed on: acting on one is nothing, and a
    /// cursor that stopped on it would be a key that did nothing.
    pub(crate) fn move_by(&mut self, delta: isize) {
        // A telemetry panel walks its own provider cursor: the rows are built
        // from that state and not the other way round.
        if let Some(panel) = self.telemetry.as_mut() {
            panel.move_cursor(delta);
            return;
        }
        // A report has no cursor: its rows are text to read, and a cursor on
        // one would be a highlight a person cannot act on. Its arrows scroll,
        // which the reader does instead.
        if !self.editable || self.rows.is_empty() {
            return;
        }
        let step = if delta < 0 { -1 } else { 1 };
        let mut left = delta.unsigned_abs();
        let mut at = self.cursor;
        while left > 0 {
            let Some(next) = at.checked_add_signed(step) else {
                break;
            };
            if next >= self.rows.len() {
                break;
            }
            at = next;
            if self.rows[at].action.is_some() && !self.rows[at].heading {
                left -= 1;
            }
        }
        if self.rows[at].action.is_some() {
            self.cursor = at;
        }
    }

    /// What a key means, and what the panel decided about it.
    ///
    /// It is a method and not a branch in the reader, so the decided behaviour
    /// is what the tests drive: the reader cannot be tested without a terminal
    /// and this can.
    pub(crate) fn key(&mut self, key: Key) -> Choice {
        let choice = self.key_without_rows(key);
        // A telemetry panel is drawn from its state, so a key that moved the
        // state moved what is on screen: the rows are rebuilt here, once, and
        // the frame only draws them.
        self.refresh_telemetry();
        choice
    }

    fn key_without_rows(&mut self, key: Key) -> Choice {
        match key {
            Key::Escape => Choice::Closed,
            Key::Up => {
                self.move_by(-1);
                Choice::None
            }
            Key::Down => {
                self.move_by(1);
                Choice::None
            }
            Key::PageUp => {
                self.move_by(-PAGE);
                Choice::None
            }
            Key::PageDown => {
                self.move_by(PAGE);
                Choice::None
            }
            // The scope is the panel's and not the caller's: a person who is
            // denying ten tools decides once where the rule goes.
            Key::ToggleScope => {
                self.scope = self.scope.flipped();
                Choice::None
            }
            Key::Enter => {
                if let Some(panel) = self.telemetry.as_mut() {
                    panel.toggle_at_cursor();
                    return Choice::None;
                }
                let Some(action) = self
                    .rows
                    .get(self.cursor)
                    .and_then(|row| row.action.clone())
                else {
                    return Choice::None;
                };
                Choice::Act {
                    action,
                    scope: self.scope,
                }
            }
            Key::Tab => {
                if let Some(panel) = self.telemetry.as_mut() {
                    panel.next_tab();
                }
                Choice::None
            }
            Key::NextRange => {
                if let Some(panel) = self.telemetry.as_mut() {
                    panel.next_range();
                }
                Choice::None
            }
            Key::PrevRange => {
                if let Some(panel) = self.telemetry.as_mut() {
                    panel.prev_range();
                }
                Choice::None
            }
            Key::NextMetric => {
                if let Some(panel) = self.telemetry.as_mut() {
                    panel.next_metric();
                }
                Choice::None
            }
            Key::PrevMetric => {
                if let Some(panel) = self.telemetry.as_mut() {
                    panel.prev_metric();
                }
                Choice::None
            }
        }
    }

    /// Moves the top of the panel, clamping at both ends.
    ///
    /// The frame calls this with the offset the keyboard asked for: what the
    /// reader sends is a number of rows, and what fits on screen is decided
    /// where the height is known.
    pub(crate) fn scroll_by(&mut self, delta: isize, height: usize) {
        let most = self.rows.len().saturating_sub(height);
        self.top = self.top.saturating_add_signed(delta).min(most);
    }

    /// Draws the panel, over everything else.
    ///
    /// It takes the middle of the screen rather than all of it: the
    /// conversation stays visible around the edges, which is what tells a
    /// person the panel can be dismissed instead of being the whole program.
    pub(crate) fn draw(&self, frame: &mut Draw<'_>, area: Rect, style: Style) {
        if self.is_empty() {
            return;
        }
        let panel = panel_area(area);
        // The clear is what makes it float: without it the conversation shows
        // through the panel and neither can be read.
        Clear.render(panel, frame.buffer_mut());

        // A border on all four sides, so the panel reads as a thing on top and
        // not as more of the conversation.
        let inner = Rect::new(
            panel.x + 2,
            panel.y + 1,
            panel.width.saturating_sub(4),
            panel.height.saturating_sub(2),
        );
        let room = usize::from(inner.height);
        let shown = self.rows.len().min(room);
        // The window follows the cursor when there is one, so the row a person
        // is walking toward is the row that is on screen. A report has no
        // cursor and reads from its top.
        let from = if self.editable {
            self.top
                .max(self.cursor.saturating_sub(room.saturating_sub(1)))
                .min(self.rows.len().saturating_sub(shown))
        } else {
            self.top.min(self.rows.len().saturating_sub(shown))
        };
        let rows: Vec<Line> = self.rows[from..from + shown]
            .iter()
            .enumerate()
            .map(|(offset, row)| {
                let under_cursor = self.editable && from + offset == self.cursor;
                row_line(row, under_cursor, style)
            })
            .collect();
        Paragraph::new(rows).render(inner, frame.buffer_mut());

        // The frame of the panel, drawn after its contents so the corners are
        // not overwritten by a long line.
        let border = style.blue_span();
        let width = usize::from(panel.width).saturating_sub(2);
        let title = format!(" {} ", crate::text::terminal(&self.title));
        let title: String = title.chars().take(width).collect();
        let fill = width.saturating_sub(title.chars().count() + 1);
        put(
            frame,
            panel.x,
            panel.y,
            &format!("╭─{title}{}╮", "─".repeat(fill)),
            border,
        );
        for row in 1..panel.height.saturating_sub(1) {
            put(frame, panel.x, panel.y + row, "│", border);
            put(
                frame,
                panel.x + panel.width.saturating_sub(1),
                panel.y + row,
                "│",
                border,
            );
        }
        // What the panel holds and how to leave it, on the bottom border: a
        // report that hid its own size is one a person cannot tell from a
        // complete one.
        put(
            frame,
            panel.x,
            panel.y + panel.height.saturating_sub(1),
            &format!("╰{}╯", self.footer(room, from, shown, width)),
            border,
        );
    }

    /// The bottom border: what the panel holds, and what a key does here.
    fn footer(&self, room: usize, from: usize, shown: usize, width: usize) -> String {
        let mut hint = String::new();
        if self.rows.len() > room {
            use std::fmt::Write as _;
            let _ = write!(
                hint,
                " {}-{} of {} ",
                from + 1,
                from + shown,
                self.rows.len()
            );
        }
        if self.editable {
            use std::fmt::Write as _;
            let _ = write!(
                hint,
                "· ↑↓ move · enter act · ^g {} · esc close ",
                self.scope.word()
            );
        } else {
            hint.push_str("· ↑↓ scroll · esc close ");
        }
        format!("{hint:─^width$}").chars().take(width).collect()
    }
}

/// How far a page key moves the cursor.
const PAGE: isize = 10;

/// One row, as the panel draws it.
fn row_line(row: &Row, under_cursor: bool, _style: Style) -> Line<'static> {
    // The escapes go before anything else, whatever path built the row: a report
    // that carried an SGR code would show it as the characters it is, and the one
    // place every row is drawn is the one place that can make that impossible
    // rather than unlikely. The strength and the notes are built from it too, so
    // the column they line up on is the text's and not the codes'.
    let mut text = crate::select::strip_style(&row.text);
    if let Some(note) = &row.note {
        // The note is a word on the right, stripped of any escape the caller
        // baked in for the same reason the row's own text is.
        let note = crate::select::strip_style(note);
        // It is padded to a fixed column, so a column of notes lines up and a
        // reader can scan it.
        let room = NOTE_COLUMN.saturating_sub(text.chars().count());
        if room > note.chars().count() {
            text.push_str(&" ".repeat(room - note.chars().count()));
            text.push_str(&note);
        } else {
            use std::fmt::Write as _;
            let _ = write!(text, "  {note}");
        }
    }
    let ink = if row.heading {
        Style::bold_span()
    } else if under_cursor {
        Style::selected()
    } else if row.dim {
        ratatui::style::Style::new().add_modifier(ratatui::style::Modifier::DIM)
    } else {
        ratatui::style::Style::new()
    };
    let lead = if under_cursor { "› " } else { "  " };
    Line::from(Span::styled(
        format!("{lead}{}", crate::text::terminal(&text)),
        ink,
    ))
}

/// How wide the text column is, so a note on the right lines up.
///
/// It is a fixed column and not the panel's width: the panel is sized from the
/// screen and a note that moved with it would be a note that moved while a
/// person was reading the list.
const NOTE_COLUMN: usize = 44;

/// Where the panel sits: the middle of the screen, most of it but not all.
///
/// A panel that took the whole screen would be indistinguishable from the
/// program having changed, and the conversation around the edges is what says
/// it can be closed.
fn panel_area(area: Rect) -> Rect {
    let [_, middle, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Percentage(80),
        Constraint::Fill(1),
    ])
    .areas(area);
    let [_, middle, _] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Percentage(90),
        Constraint::Fill(1),
    ])
    .areas(middle);
    middle
}

/// Writes one run of text at a position, which is what a border is.
fn put(frame: &mut Draw<'_>, x: u16, y: u16, text: &str, style: ratatui::style::Style) {
    let buffer = frame.buffer_mut();
    let area = buffer.area;
    if y >= area.height || x >= area.width {
        return;
    }
    // The columns the row has left are what bounds the text; anything past
    // them has nowhere to go.
    for (character, column) in text.chars().zip(x..area.width) {
        let cell = &mut buffer[(column, y)];
        cell.set_char(character);
        cell.set_style(style);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An editor over three tools, which is the shape `/tools` builds.
    fn tools() -> Modal {
        Modal::editable(
            "tools",
            vec![
                Row::heading("rules"),
                Row::actable("read", Action::Deny("read".to_owned())).noted("allow"),
                Row::actable("shell", Action::Allow("shell".to_owned())).noted("deny"),
                Row::actable("write", Action::Allow("write".to_owned())).noted("deny"),
            ],
        )
    }

    #[test]
    fn the_panel_takes_most_of_the_screen_and_not_all_of_it() {
        // A panel that covered everything would be indistinguishable from the
        // program having changed, and the conversation around the edges is what
        // says it can be closed.
        let screen = Rect::new(0, 0, 100, 40);
        let panel = panel_area(screen);
        assert!(panel.width < screen.width, "{panel:?}");
        assert!(panel.height < screen.height, "{panel:?}");
        assert!(panel.width > screen.width / 2, "{panel:?}");
        assert!(panel.height > screen.height / 2, "{panel:?}");
    }

    #[test]
    fn an_empty_panel_is_not_drawn() {
        // A command that reported nothing is not a panel with nothing in it: a
        // frame around an empty box says something went wrong.
        assert!(Modal::report("nothing", Vec::new()).is_empty());
    }

    #[test]
    fn the_cursor_starts_on_a_row_that_can_be_acted_on() {
        // A heading is drawn and never landed on: a cursor that started on one
        // would make Enter do nothing on the first thing a person sees.
        let modal = tools();
        assert_eq!(modal.cursor, 1, "the first actable row, not the heading");
        assert_eq!(
            modal.rows[modal.cursor].action,
            Some(Action::Deny("read".to_owned()))
        );
    }

    #[test]
    fn the_cursor_skips_headings_and_holds_at_both_ends() {
        let mut modal = Modal::editable(
            "t",
            vec![
                Row::heading("h"),
                Row::actable("a", Action::Allow("a".to_owned())),
                Row::heading("h2"),
                Row::actable("b", Action::Allow("b".to_owned())),
            ],
        );
        assert_eq!(modal.cursor, 1);
        modal.move_by(1);
        assert_eq!(modal.cursor, 3, "the heading is stepped over");
        // And it holds: a key at the end does not wrap, because a row that
        // depends on how many times a key was pressed is a row nobody can aim
        // at.
        modal.move_by(1);
        assert_eq!(modal.cursor, 3);
        modal.move_by(-1);
        assert_eq!(modal.cursor, 1);
        modal.move_by(-1);
        assert_eq!(modal.cursor, 1);
    }

    #[test]
    fn enter_acts_on_the_row_under_the_cursor_and_carries_the_scope() {
        // The panel does not know what allowing a tool does: what it produces is
        // the intent, and the loop that owns the service is what acts on it.
        let mut modal = tools();
        assert_eq!(
            modal.key(Key::Enter),
            Choice::Act {
                action: Action::Deny("read".to_owned()),
                scope: Scope::Session,
            }
        );
        // The scope is the panel's, and it is carried by the decision.
        assert_eq!(modal.key(Key::ToggleScope), Choice::None);
        assert_eq!(modal.scope, Scope::Global);
        assert_eq!(
            modal.key(Key::Enter),
            Choice::Act {
                action: Action::Deny("read".to_owned()),
                scope: Scope::Global,
            }
        );
    }

    #[test]
    fn a_report_has_no_cursor_and_no_actions() {
        // A report and an editor are one widget: a report has no actable rows,
        // so Enter is nothing and the cursor never moves.
        let mut modal = Modal::report("tools", vec!["a".to_owned(), "b".to_owned()]);
        assert!(!modal.editable);
        assert_eq!(modal.key(Key::Down), Choice::None);
        assert_eq!(modal.cursor, 0);
        assert_eq!(modal.key(Key::Enter), Choice::None);
        assert_eq!(modal.key(Key::Escape), Choice::Closed);
    }

    #[test]
    fn scrolling_clamps_at_both_ends() {
        let mut modal = Modal::report("tools", (0..100).map(|n| format!("line {n}")).collect());
        modal.scroll_by(-1_000, 10);
        assert_eq!(modal.top, 0, "there is nothing above the first line");
        modal.scroll_by(10_000, 10);
        assert_eq!(
            modal.top, 90,
            "100 lines, ten shown: ninety is as far as the top goes"
        );
    }
}
