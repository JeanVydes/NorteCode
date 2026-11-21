//! Painting for the plan review frame, buttons and key hints.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use terminalui::Theme;
use terminalui::components::chrome::{Border, BorderSet, Chrome, Padding, Radius, draw_box};

use super::document::{self, DocumentState};
use super::{Mode, PlanOverlay};

impl PlanOverlay {
    pub(super) fn paint(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let Some(plan) = &self.plan else {
            return;
        };
        self.area = area;
        paint_scrim(buf, area, theme);
        let inner = draw_box(buf, dialog(area), &chrome(theme), theme);
        let title = if self.question.is_some() {
            format!(" {} ", crate::ui::copy::PLAN_REVIEW)
        } else {
            format!(" {} ", plan.title.as_deref().unwrap_or(&plan.id))
        };
        paint_title(buf, inner, theme, &title);

        let mut state = DocumentState {
            doc: &mut self.doc,
            width: &mut self.width,
            theme: &mut self.doc_theme,
            scroll: &mut self.scroll,
            body: &mut self.body,
        };
        let layout = document::layout(inner, &plan.content, theme, &mut state);
        let review = self.question.is_some();
        paint_header(buf, inner, theme, plan.content.lines().count(), review);
        document::paint(buf, inner, theme, &self.doc, &layout, self.scroll);
        paint_buttons(buf, inner, theme, layout.footer, review, &mut self.buttons);
        paint_controls(
            buf,
            inner,
            theme,
            layout.footer,
            review,
            self.mode,
            &self.response,
        );
    }
}

fn dialog(area: Rect) -> Rect {
    let width = area.width.saturating_mul(88) / 100;
    let height = area.height.saturating_mul(84) / 100;
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

fn chrome(theme: &Theme) -> Chrome {
    Chrome {
        border: Some(Border {
            set: BorderSet::Rounded,
            color: Some(theme.border),
            radius: Radius::Small,
        }),
        background: Some(theme.secondary),
        radius: Radius::Small,
        padding: Padding {
            horizontal: 0,
            vertical: 0,
        },
        border_gradient: None,
        fill_gradient: None,
    }
}

fn paint_scrim(buf: &mut Buffer, area: Rect, theme: &Theme) {
    let style = Style::default().bg(theme.background).fg(theme.muted);
    let blank = " ".repeat(usize::from(area.width));
    for y in area.y..area.y + area.height {
        buf.set_stringn(area.x, y, &blank, usize::from(area.width), style);
    }
}

fn paint_title(buf: &mut Buffer, inner: Rect, theme: &Theme, title: &str) {
    buf.set_stringn(
        inner.x + 1,
        inner.y,
        title,
        usize::from(inner.width.saturating_sub(2)),
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    );
}

fn paint_header(buf: &mut Buffer, inner: Rect, theme: &Theme, lines: usize, review: bool) {
    let hint = if review {
        "  j/k scroll, g/G top and bottom"
    } else {
        "  Esc closes"
    };
    buf.set_stringn(
        inner.x + 1,
        inner.y + 1,
        format!("{lines} lines{hint}"),
        usize::from(inner.width.saturating_sub(2)),
        Style::default().fg(theme.muted),
    );
}

fn button_label(index: usize) -> &'static str {
    match index {
        0 => "accept",
        1 => "decline",
        _ => crate::ui::copy::FEEDBACK,
    }
}

fn paint_buttons(
    buf: &mut Buffer,
    inner: Rect,
    theme: &Theme,
    y: u16,
    review: bool,
    buttons: &mut Vec<(Rect, usize)>,
) {
    buttons.clear();
    let mut x = inner.x + 1;
    for index in 0..3 {
        let label = button_label(index);
        let width = u16::try_from(label.chars().count() + 2).unwrap_or(0);
        let rect = Rect {
            x,
            y,
            width,
            height: 1,
        };
        buf.set_stringn(
            rect.x,
            rect.y,
            format!(" {label} "),
            usize::from(width),
            button_style(theme, index, review),
        );
        buttons.push((rect, index));
        x = x.saturating_add(width).saturating_add(2);
    }
}

fn button_style(theme: &Theme, index: usize, review: bool) -> Style {
    let style = Style::default().add_modifier(Modifier::BOLD);
    if !review {
        return style.fg(theme.foreground).bg(theme.background);
    }
    if index == 0 {
        style.fg(theme.background).bg(theme.accent)
    } else {
        style.fg(theme.foreground).bg(theme.muted)
    }
}

fn paint_controls(
    buf: &mut Buffer,
    inner: Rect,
    theme: &Theme,
    footer_top: u16,
    review: bool,
    mode: Mode,
    response: &str,
) {
    let y = footer_top + 1;
    let text = if mode == Mode::Feedback {
        format!("{response}_")
    } else if review {
        "a/Enter accept, d decline, f feedback, j/k scroll".to_owned()
    } else {
        "Esc closes".to_owned()
    };
    let color = if mode == Mode::Feedback {
        theme.foreground
    } else {
        theme.muted
    };
    buf.set_stringn(
        inner.x + 1,
        y,
        text,
        usize::from(inner.width.saturating_sub(2)),
        Style::default().fg(color),
    );
    if mode == Mode::Feedback {
        buf.set_stringn(
            inner.x + 1,
            y + 1,
            "Enter sends, Esc cancels",
            usize::from(inner.width.saturating_sub(2)),
            Style::default().fg(theme.muted),
        );
    }
}
