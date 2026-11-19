//! The view tree: header, rail, stream, composer, status strip, pages.

mod composer;
#[cfg(test)]
#[path = "focus_tests.rs"]
mod focus_tests;
mod header;
mod inspector;
mod key_prompt;
mod pages;
mod question_prompt;
mod rail;
mod selector;
mod transcript;

use ratatui::layout::Constraint;
use terminalui::Pane;

pub(crate) use composer::Composer;
pub(crate) use header::Header;
pub(crate) use inspector::Inspector;
pub(crate) use key_prompt::{KeyPrompt, provider_has_key};
pub(crate) use question_prompt::QuestionPrompt;
pub(crate) use rail::Rail;
pub(crate) use selector::Selector;
pub(crate) use transcript::{Item, Tone, ToolState, Transcript};

use crate::theme;
use crate::ui_model::{Tab, UiModel};

use terminalui::{Stack, Text, TextStyle, Tree, node};

use terminalui::components::statusbar::{StatusBar, StatusBarStyle};

fn tab_index(tab: Tab) -> usize {
    match tab {
        Tab::Chat => 0,
        Tab::Sessions => 1,
        Tab::Models => 2,
        Tab::Plans => 3,
        Tab::Jobs => 4,
        Tab::Telemetry => 5,
        Tab::Host => 6,
        Tab::Plugins => 7,
        Tab::Costs => 8,
    }
}

pub(crate) fn build_tree(model: &UiModel) -> Tree {
    let theme = theme::theme(model.preferences.theme);

    let status_style = StatusBarStyle {
        background: Some(theme.secondary),
        separator: Some(theme.border),
    };

    let chat = node(
        "chat",
        Stack::horizontal()
            .constraints(vec![Constraint::Length(28), Constraint::Fill(1)])
            .children(vec![
                node("rail", rail::Rail::detached()),
                node("stream", transcript::Transcript::new()),
            ]),
    );

    let middle = node(
        "middle",
        Pane::new(vec![
            chat,
            node("sessions", pages::session(model)),
            node("models", pages::models(model)),
            node("plans", pages::plans(model)),
            node("jobs", pages::jobs(model)),
            node("telemetry", pages::telemetry(model)),
            node("host", pages::host(model)),
            node("plugins", pages::plugins(model)),
            node("costs", pages::costs(model)),
        ]),
    );

    let frame = node(
        "frame",
        Stack::vertical()
            .constraints(vec![
                Constraint::Length(1),
                Constraint::Fill(1),
                Constraint::Length(6),
                Constraint::Length(1),
            ])
            .children(vec![
                node(
                    "header",
                    header::Header::new(model.turn, model.preferences.theme.name().to_string()),
                ),
                middle,
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
                                        crate::ui_model::ReasoningLevel::Auto => 0,
                                        crate::ui_model::ReasoningLevel::Low => 1,
                                        crate::ui_model::ReasoningLevel::Medium => 2,
                                        crate::ui_model::ReasoningLevel::High => 3,
                                    },
                                ),
                            ),
                        ])
                        .right(vec![
                            node(
                                "hint",
                                Text::new("⏎ send · ⇧⏎ newline")
                                    .with_style(TextStyle::default().fg(theme.muted)),
                            ),
                            node(
                                "state",
                                Text::new("● ready")
                                    .with_style(TextStyle::default().fg(theme.success).bold()),
                            ),
                        ])
                        .with_style(status_style),
                ),
            ]),
    );

    let root = node(
        "root",
        Pane::new(vec![
            frame,
            node("key_prompt", key_prompt::KeyPrompt::new()),
            node("question_prompt", question_prompt::QuestionPrompt::new()),
        ]),
    );

    let mut tree = Tree::new(root);
    tree.focus("composer");
    tree
}

/// Sync the pane visibility + composer disabled state + tab to the model's tab.
///
/// The key prompt and the ask-user prompt each replace the whole frame while
/// they are open; the frame returns when they close.
pub(crate) fn sync_views(tree: &mut Tree, model: &crate::ui_model::UiModel) {
    if let Some(pane) = tree.find_mut::<Pane>("middle") {
        pane.set_visible(tab_index(model.tab));
    }
    if let Some(composer) = tree.find_mut::<Composer>("composer") {
        composer.set_disabled(model.tab != Tab::Chat);
    }
    if let Some(header) = tree.find_mut::<Header>("header") {
        header.tab = model.tab;
    }
    let prompt_open = tree
        .find::<key_prompt::KeyPrompt>("key_prompt")
        .is_some_and(key_prompt::KeyPrompt::is_open);
    let question_open = model.question.is_some();
    if let Some(root) = tree.find_mut::<Pane>("root") {
        root.set_visible(if question_open {
            2
        } else {
            usize::from(prompt_open)
        });
    }
    if let Some(overlay) = tree.find_mut::<question_prompt::QuestionPrompt>("question_prompt") {
        let open = overlay.is_open();
        if question_open && !open {
            if let Some(question) = model.question.clone() {
                overlay.begin(question);
            }
        } else if !question_open && open {
            overlay.close();
        }
    }
}

/// Refresh the inspectors from the latest model snapshot; called after data arrives.
pub(crate) fn refresh_views(tree: &mut Tree, model: &UiModel) {
    if let Some(inspector) = tree.find_mut::<inspector::Inspector>("sessions") {
        *inspector = pages::session(model);
    }
    if let Some(inspector) = tree.find_mut::<inspector::Inspector>("models") {
        *inspector = pages::models(model);
    }
    if let Some(inspector) = tree.find_mut::<inspector::Inspector>("plans") {
        *inspector = pages::plans(model);
    }
    if let Some(inspector) = tree.find_mut::<inspector::Inspector>("jobs") {
        *inspector = pages::jobs(model);
    }
    if let Some(inspector) = tree.find_mut::<inspector::Inspector>("telemetry") {
        *inspector = pages::telemetry(model);
    }
    if let Some(inspector) = tree.find_mut::<inspector::Inspector>("host") {
        *inspector = pages::host(model);
    }
    if let Some(inspector) = tree.find_mut::<inspector::Inspector>("plugins") {
        *inspector = pages::plugins(model);
    }
    if let Some(inspector) = tree.find_mut::<inspector::Inspector>("costs") {
        *inspector = pages::costs(model);
    }
}
