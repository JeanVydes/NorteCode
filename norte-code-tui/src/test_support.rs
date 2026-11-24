//! Shared fixtures and helpers for the TUI tests.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use terminalui::{Theme, Tree};

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
