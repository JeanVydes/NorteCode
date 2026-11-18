//! The view tree: header, rail, stream, composer, status strip.

mod composer;
mod header;
mod rail;
mod selector;
mod transcript;

pub(crate) use composer::Composer;
pub(crate) use header::Header;
pub(crate) use rail::Rail;
pub(crate) use transcript::{Item, ToolState, Tone, Transcript};

use ratatui::layout::Constraint;
use terminalui::components::statusbar::{StatusBar, StatusBarStyle};
use terminalui::{Text, TextStyle, Tree, Stack, node};

use crate::ui_model::UiModel;
use crate::{ui_model, theme};

pub(crate) fn build_tree(model: &UiModel) -> Tree {
    let theme = theme::theme(model.preferences.theme);

    let selector_style = StatusBarStyle {
        background: Some(theme.secondary),
        separator: Some(theme.border),
    };

    let root = node(
        "root",
        Stack::vertical()
            .constraints(vec![
                Constraint::Length(1),
                Constraint::Fill(1),
                Constraint::Length(4),
                Constraint::Length(1),
            ])
            .children(vec![
                node(
                    "header",
                    header::Header::new(model.turn, model.preferences.theme.name().to_string()),
                ),
                node(
                    "middle",
                    Stack::horizontal()
                        .constraints(vec![Constraint::Length(28), Constraint::Fill(1)])
                        .children(vec![
                            node("rail", rail::Rail::detached()),
                            node("stream", transcript::Transcript::new()),
                        ]),
                ),
                node("composer", composer::Composer::new(&theme)),
                node(
                    "status",
                    StatusBar::new()
                        .left(vec![
                            node(
                                "queue",
                                selector::Selector::new(
                                    "queue",
                                    vec!["steer", "queue", "interrupt", "reject"],
                                    0,
                                ),
                            ),
                            node(
                                "reasoning",
                                selector::Selector::new(
                                    "reasoning",
                                    vec!["auto", "low", "medium", "high"],
                                    match model.preferences.reasoning {
                                        ui_model::ReasoningLevel::Auto => 0,
                                        ui_model::ReasoningLevel::Low => 1,
                                        ui_model::ReasoningLevel::Medium => 2,
                                        ui_model::ReasoningLevel::High => 3,
                                    },
                                ),
                            ),
                        ])
                        .right(vec![
                            node(
                                "hint",
                                Text::new("⌃⏎ send").with_style(TextStyle::default().fg(theme.muted)),
                            ),
                            node(
                                "state",
                                Text::new("● ready")
                                    .with_style(TextStyle::default().fg(theme.success).bold()),
                            ),
                        ])
                        .with_style(selector_style),
                ),
            ]),
    );

    let mut tree = Tree::new(root);
    tree.focus("composer");
    tree
}
