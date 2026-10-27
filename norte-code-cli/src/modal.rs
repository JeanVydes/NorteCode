//! The modal: a panel of text over the interface, on its own.
//!
//! A command that reports something — `/tools`, `/debug`, `/jobs` — used to
//! print its lines onto the terminal. The frame owns the terminal now, so those
//! lines landed on top of the conversation: that is the overlap, and it is not
//! a rendering bug to patch but a place the text was never meant to go.
//!
//! What a report is, is a panel: lines the person asked for, over the
//! interface, read and dismissed. It is drawn last so it covers what is under
//! it, Escape closes it, and it is bounded — a report longer than the terminal
//! scrolls inside it and says how much it holds, which is the rule every other
//! bound in this client follows.
//!
//! It is deliberately not a widget library. A plugin that wants to draw its own
//! panel needs the state and the area, and that is what this is: plain lines and
//! one place that draws them.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Widget as _};

use crate::style::Style;
use crate::terminal::Draw;

/// A panel of text over the interface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Modal {
    /// What the panel is about, on its top border.
    pub(crate) title: String,
    /// The lines, already plain text.
    pub(crate) lines: Vec<String>,
    /// Which line is at the top of the panel.
    ///
    /// It counts from the beginning, which is what a report is read from: the
    /// first line is the first thing it says, and a panel that opened at its
    /// own end would have the person scrolling backwards to find out what they
    /// asked for.
    pub(crate) top: usize,
}

impl Modal {
    /// A panel with a title and some lines.
    #[must_use]
    pub(crate) fn new(title: impl Into<String>, lines: Vec<String>) -> Self {
        Self {
            title: title.into(),
            lines,
            top: 0,
        }
    }

    /// Whether there is anything to draw.
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Moves the top of the panel, clamping at both ends.
    ///
    /// The frame calls this with the offset the keyboard asked for: what the
    /// reader sends is a number of lines, and what fits on screen is decided
    /// where the height is known.
    #[cfg(test)]
    pub(crate) fn scroll_by(&mut self, delta: isize, height: usize) {
        let most = self.lines.len().saturating_sub(height);
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
        // From the beginning: a report is read from the top, where the thing it
        // is about is named. Opening at the end would have the person scrolling
        // backwards to find out what they asked for.
        let shown = self.lines.len().min(room);
        let from = self.top.min(self.lines.len().saturating_sub(shown));
        let rows: Vec<Line> = self.lines[from..from + shown]
            .iter()
            .map(|line| {
                // The cursor marks the row the report is about, which is the
                // one convention the reports already wrote.
                let ink = if line.starts_with('›') {
                    style.dim_span()
                } else {
                    ratatui::style::Style::new()
                };
                Line::from(Span::styled(crate::text::terminal(line).into_owned(), ink))
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
        let hint = if self.lines.len() > room {
            format!(
                " {}-{} of {} · ↑↓ scroll · esc close ",
                from + 1,
                from + shown,
                self.lines.len()
            )
        } else {
            " esc close ".to_owned()
        };
        let hint = format!("{hint:─^width$}");
        put(
            frame,
            panel.x,
            panel.y + panel.height.saturating_sub(1),
            &format!("╰{}╯", hint.chars().take(width).collect::<String>()),
            border,
        );
    }
}

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
    fn scrolling_clamps_at_both_ends() {
        let mut modal = Modal::new("tools", (0..100).map(|n| format!("line {n}")).collect());
        // Down is towards the end, and there is nowhere past it.
        modal.scroll_by(-1_000, 10);
        assert_eq!(modal.top, 0, "there is nothing above the first line");
        // And the count of what the panel holds bounds the other way.
        modal.scroll_by(10_000, 10);
        assert_eq!(
            modal.top, 90,
            "100 lines, ten shown: ninety is as far as the top goes"
        );
    }

    #[test]
    fn an_empty_panel_is_not_drawn() {
        // A command that reported nothing is not a panel with nothing in it: a
        // frame around an empty box says something went wrong.
        assert!(Modal::new("nothing", Vec::new()).is_empty());
    }
}
