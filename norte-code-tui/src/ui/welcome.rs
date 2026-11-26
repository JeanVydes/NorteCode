use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};
use std::time::Duration;
use terminalui::{Animation, Easing, Effect, Theme};

#[derive(Default)]
pub(super) struct Welcome {
    visible: bool,
    exiting: Option<Animation>,
    elapsed: Duration,
}

impl Welcome {
    pub(super) fn set_visible(&mut self, visible: bool) {
        if self.visible && !visible {
            self.exiting = Some(Animation::new(Duration::from_millis(650), Easing::Linear));
        } else if visible {
            self.exiting = None;
        }
        self.visible = visible;
    }
    pub(super) fn animating(&self) -> bool {
        self.visible || self.exiting.is_some()
    }
    pub(super) fn tick(&mut self, delta: Duration) {
        self.elapsed = self.elapsed.saturating_add(delta);
        if self
            .exiting
            .as_mut()
            .is_some_and(|animation| !animation.tick(delta))
        {
            self.exiting = None;
        }
    }
    pub(super) fn paint(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        if !self.animating() || area.width < 12 || area.height < 5 {
            return;
        }
        let width = area.width.min(60);
        let height = area.height.min(7);
        let rect = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        let paint = |area: Rect, buf: &mut Buffer, theme: &Theme| {
            let star = ["◇", "◈", "◆", "◈"][(self.elapsed.as_millis() / 350 % 4) as usize];
            let lines = [
                (format!("{star}  NORTE CODE"), theme.accent),
                ("Start a new conversation".to_owned(), theme.foreground),
                ("Send a message to create a session".to_owned(), theme.muted),
                ("Use /open to continue a session".to_owned(), theme.muted),
            ];
            for (index, (text, color)) in lines.iter().enumerate() {
                let width = unicode_width::UnicodeWidthStr::width(text.as_str())
                    .min(usize::from(area.width));
                let x = area.x
                    + (area
                        .width
                        .saturating_sub(u16::try_from(width).unwrap_or(u16::MAX)))
                        / 2;
                let y = area.y + 1 + u16::try_from(index).unwrap_or(u16::MAX);
                if y < area.bottom() {
                    buf.set_stringn(
                        x,
                        y,
                        text,
                        width,
                        Style::default().fg(*color).add_modifier(if index == 0 {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                    );
                }
            }
        };
        if let Some(animation) = &self.exiting {
            terminalui::render_with(
                &Effect::Thanos { seed: 42 },
                animation.progress(),
                buf,
                rect,
                theme,
                paint,
            );
        } else {
            paint(rect, buf, theme);
        }
    }
}
