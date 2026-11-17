//! The tab bar: the whole chrome at the top of the screen.
//!
//! One row holds the wordmark, every tab and, pushed to the right edge, the
//! session and the model. The bar measures in display width and drops what
//! does not fit instead of letting a narrow terminal cut it in half.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use crate::components::{ComponentId, palette};
use crate::ui_model::{ThemeMode, UiModel};

/// The space between two tabs.
const TAB_GAP: usize = 2;

impl super::TextBuilder {
    /// The bar, with the clickable spans and the columns they cover.
    #[must_use]
    pub(crate) fn tab_bar(model: &UiModel, active: ComponentId, width: u16) -> TabBar {
        let theme = model.preferences.theme;
        let budget = usize::from(width);
        let mut bar = TabBar::new(theme, budget);

        bar.push_muted(" ");
        for tab in ComponentId::tabs() {
            let selected = tab == active;
            let label = Self::tab_text(tab);
            let separator = if bar.spans.is_empty() { "" } else { "  " };
            let used = bar.used + TAB_GAP;
            if used + UnicodeWidthStr::width(label.as_str()) > budget {
                break;
            }
            bar.push_muted(separator);
            let start = bar.used;
            bar.push_styled(label, tab_style(theme, selected));
            bar.tabs.push((start, bar.used, tab));
        }

        bar.push_right(&Self::tab_meta(model));
        bar.finish()
    }

    /// The text of one tab: its digit and its name.
    fn tab_text(tab: ComponentId) -> String {
        match tab.shortcut() {
            Some(digit) => format!("{digit} {}", Self::view_label(tab)),
            None => Self::view_label(tab).to_owned(),
        }
    }

    /// What the right edge of the bar says: the session and the model.
    fn tab_meta(model: &UiModel) -> String {
        let session = model
            .active_session
            .as_ref()
            .map(|session| session.as_str().chars().take(8).collect::<String>());
        let model_name = model
            .session_model
            .as_ref()
            .map(|model| format!("{}/{}", model.provider, model.model))
            .or_else(|| {
                let active = model.active_session.as_ref()?;
                let summary = model
                    .sessions
                    .iter()
                    .find(|summary| summary.session_id == active.as_str())?;
                Some(format!(
                    "{}/{}",
                    summary.provider.clone()?,
                    summary.model.clone()?.id
                ))
            });
        [session, model_name]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("  ")
    }
}

/// The style a tab is drawn with.
fn tab_style(theme: ThemeMode, selected: bool) -> Style {
    if selected {
        palette::active_tab_style(theme)
    } else {
        palette::inactive_tab_style(theme)
    }
}

/// The bar as it is drawn, with the tab spans measured in columns.
pub(crate) struct TabBar {
    pub(crate) line: Line<'static>,
    /// The clickable tabs, with the columns each one spans.
    pub(crate) tabs: Vec<(usize, usize, ComponentId)>,
    spans: Vec<Span<'static>>,
    used: usize,
    budget: usize,
    theme: ThemeMode,
}

impl TabBar {
    fn new(theme: ThemeMode, budget: usize) -> Self {
        Self {
            line: Line::default(),
            tabs: Vec::new(),
            spans: Vec::new(),
            used: 0,
            budget,
            theme,
        }
    }

    fn push_styled(&mut self, text: String, style: Style) {
        self.used = self
            .used
            .saturating_add(UnicodeWidthStr::width(text.as_str()));
        self.spans.push(Span::styled(text, style));
    }

    fn push_muted(&mut self, text: &str) {
        self.push_styled(
            text.to_owned(),
            Style::default().fg(palette::text_muted(self.theme)),
        );
    }

    /// Pushes the meta to the right edge, without crossing the tabs.
    fn push_right(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let padded = format!("{text} ");
        let width = UnicodeWidthStr::width(padded.as_str());
        let room = self.budget.saturating_sub(self.used).saturating_sub(1);
        if width > room {
            return;
        }
        let fill = room.saturating_sub(width);
        self.push_muted(&" ".repeat(fill));
        self.spans.push(Span::styled(
            padded,
            Style::default()
                .fg(palette::text_muted(self.theme))
                .add_modifier(Modifier::DIM),
        ));
        self.used = self.used.saturating_add(width).saturating_add(fill);
    }

    fn finish(self) -> Self {
        Self {
            line: Line::from(self.spans.clone()),
            ..self
        }
    }
}
