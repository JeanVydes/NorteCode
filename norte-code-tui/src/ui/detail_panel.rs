//! A bordered panel of labelled values, used beside the catalogue lists.

use std::any::Any;

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use serde::Serialize;

use terminalui::components::badge::{Badge, BadgeVariant};
use terminalui::components::chrome::Padding;
use terminalui::components::panel::Panel;
use terminalui::components::separator::Separator;
use terminalui::{Context, InputEvent, Outcome, Stack, Text, TextStyle, Theme, View, node};

/// How a field paints its value.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) enum FieldPresentation {
    /// A plain bright value.
    Text,
    /// A coloured status pill.
    Badge(BadgeVariant),
    /// A hairline between field groups; carries no label or value.
    Separator,
}

/// One labelled value in the panel.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
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

        let label_width = self
            .fields
            .iter()
            .filter(|field| !matches!(field.presentation, FieldPresentation::Separator))
            .map(|field| field.label.chars().count())
            .max()
            .unwrap_or(0)
            .min(10);
        let label_cells = u16::try_from(label_width).unwrap_or(u16::MAX);

        let mut children = vec![
            node(
                "title",
                Text::new(&self.title).with_style(TextStyle::default().fg(theme.accent).bold()),
            ),
            node("title_gap", Text::new("")),
        ];
        let mut constraints = vec![Constraint::Length(1), Constraint::Length(1)];

        for (index, field) in self.fields.iter().enumerate() {
            let id = format!("field-{index}");
            if matches!(field.presentation, FieldPresentation::Separator) {
                children.push(node(id, Separator::new()));
                constraints.push(Constraint::Length(1));
                continue;
            }

            let label = node(
                format!("{id}-label"),
                Text::new(&field.label).with_style(TextStyle {
                    foreground: Some(theme.muted),
                    background: Some(theme.background),
                    ..TextStyle::default()
                }),
            );
            let value = match &field.presentation {
                FieldPresentation::Badge(variant) => node(
                    format!("{id}-value"),
                    Badge::new(field.value.as_str()).with_variant(*variant),
                ),
                FieldPresentation::Text | FieldPresentation::Separator => node(
                    format!("{id}-value"),
                    Text::new(&field.value).with_style(TextStyle {
                        foreground: Some(theme.foreground),
                        background: Some(theme.background),
                        ..TextStyle::default()
                    }),
                ),
            };
            children.push(node(
                id,
                Stack::horizontal()
                    .children(vec![label, value])
                    .constraints(vec![
                        Constraint::Length(label_cells + 1),
                        Constraint::Fill(1),
                    ]),
            ));
            constraints.push(Constraint::Length(1));
        }

        let rows = Stack::vertical()
            .children(children)
            .constraints(constraints);
        let mut panel = Panel::themed(
            theme,
            Some(theme.border),
            None,
            Padding {
                horizontal: 1,
                vertical: 0,
            },
        )
        .children(vec![node("rows", rows)]);
        panel.render(area, buf, cx, theme);
    }

    fn handle_event(&mut self, _event: &InputEvent, _cx: &mut Context) -> Outcome {
        Outcome::Ignored
    }
}
