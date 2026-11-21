//! A bordered panel of labelled values, used beside the catalogue lists.

use std::any::Any;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use serde::Serialize;

use terminalui::components::badge::{Badge, BadgeVariant};
use terminalui::components::chrome::{Border, BorderSet, Chrome, Padding, Radius, draw_box};
use terminalui::components::separator::Separator;
use terminalui::{Context, InputEvent, Outcome, Theme, View};

/// How a field paints its value.
#[derive(Clone, Debug, Serialize)]
pub(crate) enum FieldPresentation {
    /// A plain bright value.
    Text,
    /// A coloured status pill.
    Badge(BadgeVariant),
    /// A hairline between field groups; carries no label or value.
    Separator,
}

/// One labelled value in the panel.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Field {
    /// The label, painted muted.
    pub(crate) label: String,
    /// The value, painted bright.
    pub(crate) value: String,
    presentation: FieldPresentation,
}

impl Field {
    /// A field from anything printable.
    pub(crate) fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            presentation: FieldPresentation::Text,
        }
    }

    /// A field whose value paints as a status pill.
    pub(crate) fn badge(
        label: impl Into<String>,
        value: impl Into<String>,
        variant: BadgeVariant,
    ) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            presentation: FieldPresentation::Badge(variant),
        }
    }

    /// A hairline between field groups.
    pub(crate) fn separator() -> Self {
        Self {
            label: String::new(),
            value: String::new(),
            presentation: FieldPresentation::Separator,
        }
    }
}

/// A read-only panel: title, then one `label value` row per field.
#[derive(Serialize)]
pub(crate) struct DetailPanel {
    /// The heading at the top of the box.
    pub(crate) title: String,
    /// The rows to show.
    pub(crate) fields: Vec<Field>,
}

impl DetailPanel {
    /// An empty panel, filled by the host.
    pub(crate) fn new() -> Self {
        Self {
            title: String::new(),
            fields: Vec::new(),
        }
    }

    /// Replace the panel's content.
    pub(crate) fn set(&mut self, title: impl Into<String>, fields: Vec<Field>) {
        self.title = title.into();
        self.fields = fields;
    }
}

impl View for DetailPanel {
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn kind(&self) -> &'static str {
        "detail_panel"
    }

    fn focusable(&self) -> bool {
        false
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut Context, theme: &Theme) {
        if area.width < 8 || area.height < 3 {
            return;
        }
        let chrome = Chrome {
            border: Some(Border {
                set: BorderSet::Rounded,
                color: Some(theme.border),
                radius: Radius::Small,
            }),
            background: None,
            radius: Radius::Small,
            padding: Padding {
                horizontal: 0,
                vertical: 0,
            },
            border_gradient: None,
            fill_gradient: None,
        };
        let inner = draw_box(buf, area, &chrome, theme);
        let mut y = inner.y;
        let title = &self.title;
        buf.set_stringn(
            inner.x + 1,
            y,
            title,
            usize::from(inner.width.saturating_sub(2)),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        );
        y += 2;
        let label_width = self
            .fields
            .iter()
            .filter(|field| !matches!(field.presentation, FieldPresentation::Separator))
            .map(|field| field.label.chars().count())
            .max()
            .unwrap_or(0)
            .min(10);
        for field in &self.fields {
            if y >= inner.y + inner.height {
                break;
            }
            if matches!(field.presentation, FieldPresentation::Separator) {
                Separator::new().render(
                    Rect {
                        x: inner.x + 1,
                        y,
                        width: inner.width.saturating_sub(2),
                        height: 1,
                    },
                    buf,
                    cx,
                    theme,
                );
                y += 1;
                continue;
            }
            let dim = Style::default().fg(theme.muted).bg(theme.background);
            buf.set_stringn(
                inner.x + 1,
                y,
                &field.label,
                label_width.saturating_add(1),
                dim,
            );
            let value_x = inner.x + 1 + u16::try_from(label_width + 1).unwrap_or(u16::MAX);
            let label_cells = u16::try_from(label_width).unwrap_or(u16::MAX);
            let budget = usize::from(inner.width.saturating_sub(label_cells.saturating_add(2)));
            let row = Rect {
                x: value_x,
                y,
                width: u16::try_from(budget).unwrap_or(u16::MAX),
                height: 1,
            };
            match &field.presentation {
                FieldPresentation::Badge(variant) => {
                    let value: String = field.value.chars().take(budget).collect();
                    Badge::new(value)
                        .with_variant(*variant)
                        .render(row, buf, cx, theme);
                }
                FieldPresentation::Text | FieldPresentation::Separator => {
                    let value: String = field.value.chars().take(budget).collect();
                    buf.set_stringn(
                        row.x,
                        row.y,
                        &value,
                        value.chars().count(),
                        Style::default().fg(theme.foreground).bg(theme.background),
                    );
                }
            }
            y += 1;
        }
    }

    fn handle_event(&mut self, _event: &InputEvent, _cx: &mut Context) -> Outcome {
        Outcome::Ignored
    }
}
