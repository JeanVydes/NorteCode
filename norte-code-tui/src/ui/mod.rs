//! The view tree: header, rail, stream, composer, status strip, pages.

mod app_frame;
mod chat_layout;
mod command_palette;
#[cfg(test)]
#[path = "command_palette_tests.rs"]
mod command_palette_tests;
mod composer;
mod config_page;
mod copy;
mod debug_overlay;
mod detail_panel;
#[cfg(test)]
#[path = "focus_tests.rs"]
mod focus_tests;
mod header;
#[cfg(test)]
#[path = "header_tests.rs"]
mod header_tests;
mod host_info;
mod key_prompt;
mod key_prompt_action;
#[cfg(test)]
#[path = "key_prompt_tests.rs"]
mod key_prompt_tests;
mod key_status;
#[cfg(test)]
#[path = "layout_tests.rs"]
mod layout_tests;
mod leaderboard_section;
mod markdown;
mod models_page;
mod node_lookup;
mod overlay_frame;
mod plan_overlay;
mod plugins_page;
mod question_prompt;
mod rail;
#[cfg(test)]
#[path = "rail_tests.rs"]
mod rail_tests;
mod scroll_drag;
mod selector;
mod session_detail;
mod sessions_page;
mod settings_layout;
mod settings_page;
#[cfg(test)]
#[path = "status_footer_tests.rs"]
mod status_footer_tests;
mod telemetry_page;
#[cfg(test)]
#[path = "telemetry_page_tests.rs"]
mod telemetry_page_tests;
mod toast_host;
#[cfg(test)]
#[path = "toast_host_tests.rs"]
mod toast_host_tests;
mod transcript;

use norte_code_client::config::RetentionConfig;
use terminalui::{Pane, Spacer};

pub(crate) use chat_layout::ChatLayout;
pub(crate) use command_palette::{CommandPalette, PaletteAction};
pub(crate) use composer::Composer;
pub(crate) use config_page::{ConfigAction, ConfigData, Setting};
pub(crate) use debug_overlay::{DebugOverlay, OverlayClock};
pub(crate) use header::Header;

pub(crate) use key_prompt::{KeyIntent, KeyPrompt, KeyPromptAction};
pub(crate) use key_status::invalidate_key;
pub(crate) use models_page::{ModelsAction, ModelsData, ModelsPage};
pub(crate) use plan_overlay::{PLAN_REVIEW_PREFIX, PlanOverlay, PlanReviewAction};
pub(crate) use plugins_page::PluginsAction;
pub(crate) use question_prompt::QuestionPrompt;
pub(crate) use rail::{Rail, RailPosition};
pub(crate) use selector::Selector;
pub(crate) use sessions_page::{SessionsAction, SessionsPage};
pub(crate) use settings_page::SettingsPage;
pub(crate) use telemetry_page::TelemetryPage;
pub(crate) use toast_host::ToastHost;
pub(crate) use transcript::{Item, MessageAction, Tone, ToolState, Transcript};

use crate::ui_model::{Tab, UiModel};

use terminalui::{Text, TextStyle, Tree, node};

use terminalui::components::statusbar::{StatusBar, StatusBarStyle};

use self::copy::{READY, ShortcutHint};

/// The chat-only bottom strip: queue, reasoning, hints and connection chip.
fn status_bar(model: &UiModel, theme: &terminalui::Theme) -> terminalui::Node {
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
                node("hint", ShortcutHint::chat(theme)),
                node(
                    "state",
                    Text::new(READY).with_style(TextStyle::default().fg(theme.success).bold()),
                ),
            ])
            .with_style(StatusBarStyle {
                background: Some(theme.secondary),
                separator: Some(theme.border),
            }),
    )
}

#[cfg(test)]
pub(crate) fn build_tree(model: &UiModel) -> Tree {
    build_tree_with_retention(model, RetentionConfig::default())
}

pub(crate) fn build_tree_with_retention(model: &UiModel, retention: RetentionConfig) -> Tree {
    let theme = model.preferences.catalog.theme(&model.preferences.theme);

    let chat = node(
        "chat",
        ChatLayout::with_retention(model.preferences.rail_position, retention),
    );

    let middle = node(
        "middle",
        Pane::new(vec![
            chat,
            node("sessions", sessions_page::SessionsPage::new()),
            node("models", models_page::ModelsPage::new()),
            node("telemetry", TelemetryPage::new()),
            node("settings", settings_page::SettingsPage::new()),
        ]),
    );

    let frame = node(
        "frame",
        app_frame::AppFrame::new(
            vec![
                node(
                    "header",
                    header::Header::new(
                        model.turn,
                        crate::theme::theme_name(&model.preferences.theme).to_string(),
                    ),
                ),
                middle,
                node(
                    "composer",
                    composer::Composer::new(&theme, crate::clipboard::system()),
                ),
                node(
                    "status_pane",
                    Pane::new(vec![
                        status_bar(model, &theme),
                        node("status_blank", Spacer::new()),
                    ]),
                ),
            ],
            model.tab == Tab::Chat,
        ),
    );

    let root = node(
        "root",
        Pane::new(vec![
            frame,
            node("key_prompt", key_prompt::KeyPrompt::new()),
            node("question_prompt", question_prompt::QuestionPrompt::new()),
            node("plan_overlay", plan_overlay::PlanOverlay::default()),
        ]),
    );

    let screen = node(
        "screen",
        overlay_frame::OverlayFrame::new(
            root,
            node(
                "toast_layer",
                overlay_frame::OverlayFrame::new(
                    node("toast", ToastHost::new()),
                    node(
                        "command_layer",
                        overlay_frame::OverlayFrame::new(
                            node("command_palette", CommandPalette::new()),
                            node(
                                "debug_overlay",
                                DebugOverlay::new(model.preferences.debug.enabled()),
                            ),
                        ),
                    ),
                ),
            ),
        ),
    );

    let mut tree = Tree::new(screen);
    if let Some(rail) = tree.find_mut::<Rail>("rail") {
        rail.set_context(model.effective_context());
    }
    tree.focus("composer");
    tree
}

/// Sync the pane visibility + composer disabled state + tab to the model's tab.
///
/// The key prompt and the ask-user prompt each replace the whole frame while
/// they are open; the frame returns when they close.
pub(crate) fn sync_views(tree: &mut Tree, model: &crate::ui_model::UiModel) {
    if let Some(pane) = tree.find_mut::<Pane>("middle") {
        pane.set_visible(model.tab.index());
    }
    let chat = model.tab == Tab::Chat;
    if let Some(frame) = tree.find_mut::<app_frame::AppFrame>("frame") {
        frame.set_show_chat_chrome(chat);
    }
    if let Some(composer) = tree.find_mut::<Composer>("composer") {
        composer.set_visible(chat);
    }
    if let Some(pane) = tree.find_mut::<Pane>("status_pane") {
        pane.set_visible(usize::from(!chat));
    }
    if let Some(header) = tree.find_mut::<Header>("header") {
        header.tab = model.tab;
    }
    let prompt_open = tree
        .find::<key_prompt::KeyPrompt>("key_prompt")
        .is_some_and(key_prompt::KeyPrompt::is_open);
    let question_open = model.question.is_some();
    let plan_open = tree
        .find::<plan_overlay::PlanOverlay>("plan_overlay")
        .is_some_and(plan_overlay::PlanOverlay::is_open);
    let visible = if plan_open {
        3
    } else if question_open {
        2
    } else {
        usize::from(prompt_open)
    };
    let overlay_changed = tree
        .find::<Pane>("root")
        .is_none_or(|root| root.visible() != visible);
    if let Some(root) = tree.find_mut::<Pane>("root") {
        root.set_visible(visible);
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
    if overlay_changed {
        let focus = match visible {
            3 => "plan_overlay",
            2 => "question_prompt",
            1 => tree
                .find::<key_prompt::KeyPrompt>("key_prompt")
                .and_then(key_prompt::KeyPrompt::focus_id)
                .unwrap_or("composer"),
            _ => "composer",
        };
        tree.focus(focus);
    }
}

fn update_sessions(tree: &mut Tree, model: &UiModel, usage: crate::session_stats::SharedUsage) {
    let active = model.active.as_ref().map(|key| key.as_str().to_string());
    if let Some(page) = tree.find_mut::<sessions_page::SessionsPage>("sessions") {
        page.set_data(model.sessions.clone(), active, usage);
    }
}

fn update_models(tree: &mut Tree, model: &UiModel) {
    let mut key_status = std::collections::HashMap::new();
    if let Some(catalog) = model.catalog.as_ref() {
        for provider in &catalog.providers {
            key_status.insert(
                provider.provider_id.clone(),
                key_status::provider_has_key(&provider.provider_id, &provider.environment_keys),
            );
        }
    }
    let active = model.active_model.as_ref().map(|agent| {
        (
            model.active_provider.clone().unwrap_or_default(),
            agent.id.clone(),
        )
    });
    if let Some(page) = tree.find_mut::<models_page::ModelsPage>("models") {
        page.set_data(ModelsData {
            models: model.models.clone(),
            key_status,
            active,
        });
    }
}

fn update_config(tree: &mut Tree, model: &UiModel) {
    if let Some(page) = tree.find_mut::<SettingsPage>("settings") {
        page.set_data(
            model,
            ConfigData {
                theme: crate::theme::theme_name(&model.preferences.theme).to_string(),
                thinking: model.preferences.reasoning.label().to_string(),
                send_mode: model.preferences.send_mode.label().to_string(),
                diff_mode: model.preferences.diff_mode.label().to_string(),
                rail_position: model.preferences.rail_position.label().to_string(),
                debug: model.preferences.debug,
            },
        );
    }
    if let Some(stream) = tree.find_mut::<Transcript>("stream") {
        stream.set_diff_mode(model.preferences.diff_mode);
    }
    if let Some(chat) = tree.find_mut::<ChatLayout>("chat") {
        chat.set_position(model.preferences.rail_position);
    }
}

/// Repaint the status strip's captured colors after a theme change.
pub(crate) fn sync_theme(tree: &mut Tree, model: &UiModel) {
    let theme = model.preferences.catalog.theme(&model.preferences.theme);
    tree.visit_mut(|node| match node.id.as_str() {
        "status" => {
            if let Some(bar) = node.view.as_any().downcast_mut::<StatusBar>() {
                bar.style = StatusBarStyle {
                    background: Some(theme.secondary),
                    separator: Some(theme.border),
                };
            }
        }
        "hint" => {
            if let Some(hint) = node.view.as_any().downcast_mut::<ShortcutHint>() {
                hint.restyle(&theme);
            }
        }
        "state" => {
            if let Some(text) = node.view.as_any().downcast_mut::<Text>() {
                text.set_style(TextStyle::default().fg(theme.success).bold());
                text.set_text(READY);
            }
        }
        _ => {}
    });
}

/// Refresh the views from the latest model snapshot; called after data arrives.
pub(crate) fn refresh_views(tree: &mut Tree, model: &UiModel) {
    let usage = crate::session_stats::SharedUsage::clone(&model.telemetry_usage);
    update_sessions(
        tree,
        model,
        crate::session_stats::SharedUsage::clone(&usage),
    );
    update_models(tree, model);
    if let Some(page) = tree.find_mut::<TelemetryPage>("telemetry") {
        page.set_report(model.telemetry.clone(), usage);
        page.set_sessions(model.sessions.clone());
    }
    update_config(tree, model);
}
