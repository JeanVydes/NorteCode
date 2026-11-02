//! Where each band of the interface lands.
//!
//! The interface is a vertical stack: the conversation takes what is left, and
//! the chrome is measured from the top of its own list downward. Measuring is
//! separate from painting on purpose. A band reports its height once, from the
//! same function the painter uses to decide what to draw, and it is handed
//! exactly those rows. The height a painter assumes and the rows it is given
//! therefore cannot disagree — which is the bug every ad-hoc `Constraint` list
//! eventually grows.
//!
//! The heights are a record and not a `Vec`: the set of bands is closed, so an
//! added band is a compile error at every match and struct literal rather than
//! a silently missing row.

use ratatui::layout::Rect;

pub(crate) mod clip;
pub(crate) mod tasks;
pub(crate) mod transcript;
pub(crate) mod spinner;

/// The rows each band asks for at one width.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Heights {
    /// The live spinner, zero when nothing is in flight.
    pub(crate) spinner: u16,
    /// The compact task row.
    pub(crate) tasks: u16,
    /// A question the model asked.
    pub(crate) question: u16,
    /// The draft, wrapped to the width and clamped.
    pub(crate) input: u16,
    /// The completion menu.
    pub(crate) completion: u16,
    /// The dimmed hint under the draft.
    pub(crate) suggestion: u16,
    /// The rule above the status.
    pub(crate) rule: u16,
    /// The status row.
    pub(crate) status: u16,
}

impl Heights {
    /// Everything below the conversation, added up.
    #[must_use]
    pub(crate) fn chrome(self) -> u16 {
        [
            self.spinner,
            self.tasks,
            self.question,
            self.input,
            self.completion,
            self.suggestion,
            self.rule,
            self.status,
        ]
        .into_iter()
        .fold(0_u16, u16::saturating_add)
    }
}

/// The area each band was given.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Areas {
    /// The conversation: everything the chrome did not take.
    pub(crate) conversation: Rect,
    /// The live spinner.
    pub(crate) spinner: Rect,
    /// The compact task row.
    pub(crate) tasks: Rect,
    /// A question the model asked.
    pub(crate) question: Rect,
    /// The draft.
    pub(crate) input: Rect,
    /// The completion menu.
    pub(crate) completion: Rect,
    /// The dimmed hint under the draft.
    pub(crate) suggestion: Rect,
    /// The rule above the status.
    pub(crate) rule: Rect,
    /// The status row.
    pub(crate) status: Rect,
}

/// Splits `area` into the conversation and the chrome.
///
/// The conversation is a fill: it gets whatever the chrome does not, and it is
/// the first thing to give up its rows when the terminal is short. The chrome
/// is allocated from its own top down; a band that does not fit is clipped, and
/// the bands after it get nothing rather than overflowing the frame.
#[must_use]
pub(crate) fn split(area: Rect, heights: Heights) -> Areas {
    let conversation = area.height.saturating_sub(heights.chrome());
    let mut y = area.y.saturating_add(conversation);
    let bottom = area.y.saturating_add(area.height);
    let mut take = |wanted: u16| {
        let height = wanted.min(bottom.saturating_sub(y));
        let rect = Rect::new(area.x, y, area.width, height);
        y = y.saturating_add(height);
        rect
    };
    Areas {
        conversation: Rect::new(area.x, area.y, area.width, conversation),
        spinner: take(heights.spinner),
        tasks: take(heights.tasks),
        question: take(heights.question),
        input: take(heights.input),
        completion: take(heights.completion),
        suggestion: take(heights.suggestion),
        rule: take(heights.rule),
        status: take(heights.status),
    }
}

use ratatui::Frame;

/// A piece of the interface that can paint itself into a rectangle.
///
/// The frame is a composition of these: each widget is handed the rectangle
/// the layout chose for it and the theme it reads from, and paints there and
/// nowhere else. A widget never measures its own height a second time — the
/// layout asks first, and the same number is what it is given.
#[allow(dead_code)]
pub(crate) trait Widget {
    /// How many rows the widget needs at `width`.
    ///
    /// Measured once, from the same data the painter uses, so the height a
    /// painter assumes and the rows it is given cannot disagree.
    #[allow(clippy::trivially_copy_pass_by_ref)]
    fn measure(&self, width: u16, theme: &crate::style::Theme) -> u16;

    /// Paints the widget into `area`.
    #[allow(clippy::trivially_copy_pass_by_ref)]
    fn draw(&self, frame: &mut Frame<'_>, area: Rect, theme: &crate::style::Theme);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(height: u16) -> Rect {
        Rect::new(0, 0, 40, height)
    }

    #[test]
    fn the_conversation_takes_what_the_chrome_does_not() {
        let heights = Heights {
            suggestion: 1,
            rule: 1,
            status: 1,
            input: 2,
            ..Heights::default()
        };
        let areas = split(area(20), heights);
        assert_eq!(areas.conversation.height, 15);
        assert_eq!(areas.input.height, 2);
        assert_eq!(areas.status.y, 19);
        assert_eq!(areas.rule.y, 18);
        // The bands are contiguous: nothing is drawn twice and no row is lost.
        assert_eq!(areas.conversation.bottom(), areas.input.y);
        assert_eq!(areas.input.bottom(), areas.suggestion.y);
        assert_eq!(areas.suggestion.bottom(), areas.rule.y);
    }

    #[test]
    fn a_short_terminal_gives_up_the_conversation_before_the_chrome() {
        let heights = Heights {
            status: 1,
            rule: 1,
            input: 3,
            ..Heights::default()
        };
        let areas = split(area(4), heights);
        assert_eq!(areas.conversation.height, 0);
        assert_eq!(
            areas.input.height, 3,
            "the chrome is clipped, not the frame"
        );
        assert_eq!(areas.status.height, 0, "what does not fit is dropped");
    }
}
