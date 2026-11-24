//! Shared fixtures and helpers for the TUI tests.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use norte_code_client::config::ReconnectConfig;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use terminalui::{InputEvent, Theme, Tree};

use crossterm::event::Event;

pub(crate) fn model_info(provider: &str, id: &str) -> norte_code_wire::ModelInfo {
    norte_code_wire::ModelInfo {
        provider: provider.to_owned(),
        model: id.to_owned(),
        name: id.to_owned(),
        description: None,
        max_context: None,
        max_output: None,
        tools: false,
        input: Vec::new(),
        output: Vec::new(),
        reasoning: norte_code_wire::ReasoningInfo::default(),
        deprecated: false,
    }
}

pub(crate) fn rendered(tree: &mut Tree) -> String {
    rendered_in(tree, Rect::new(0, 0, 80, 24), &Theme::default())
}

pub(crate) fn rendered_in(tree: &mut Tree, area: Rect, theme: &Theme) -> String {
    let mut buffer = Buffer::empty(area);
    tree.render(area, &mut buffer, theme);
    let mut painted = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            painted.push_str(buffer[(x, y)].symbol());
        }
    }
    painted
}

pub(crate) fn provider_key_status(
    source: norte_code_wire::ProviderKeySource,
    environment_keys: &[&str],
) -> crate::ui::ProviderKeyStatus {
    crate::ui::ProviderKeyStatus {
        available: !matches!(
            source,
            norte_code_wire::ProviderKeySource::Missing
                | norte_code_wire::ProviderKeySource::Unknown
        ),
        source,
        environment_keys: environment_keys
            .iter()
            .map(|key| (*key).to_owned())
            .collect(),
    }
}

pub(crate) fn key_event(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent {
        code,
        modifiers,
        kind: KeyEventKind::Press,
        state: KeyEventState::empty(),
    }
}

pub(crate) fn input_event(code: KeyCode, modifiers: KeyModifiers) -> InputEvent {
    InputEvent::from(Event::Key(key_event(code, modifiers)))
}

pub(crate) fn reconnect_config(retry_base_ms: u64, retry_max_ms: u64) -> ReconnectConfig {
    ReconnectConfig {
        retry_base_ms,
        retry_max_ms,
        give_up_attempts: 3,
        healthy_stream_s: 10,
    }
}
