//! The chrome measures in display width and never overflows.

use crate::components::ComponentId;
use crate::text_builder::TextBuilder;
use crate::text_builder::status::status_line;
use crate::ui_model::{SessionStatsSummary, ThemeMode, UiModel};

fn width(line: &ratatui::text::Line<'_>) -> usize {
    line.spans
        .iter()
        .map(|span| unicode_width::UnicodeWidthStr::width(span.content.as_ref()))
        .sum()
}

fn text_of(line: &ratatui::text::Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
}

#[test]
fn the_tab_bar_fits_a_narrow_terminal() {
    let model = UiModel::default();

    for columns in [40_u16, 80, 200] {
        let bar = TextBuilder::tab_bar(&model, ComponentId::Session, columns);
        assert!(
            width(&bar.line) <= usize::from(columns),
            "the bar overflows {columns} columns"
        );
        for (_, end, _) in &bar.tabs {
            assert!(*end <= usize::from(columns));
        }
    }
}

#[test]
fn a_narrow_bar_drops_tabs_instead_of_cutting_them() {
    let model = UiModel::default();
    let narrow = TextBuilder::tab_bar(&model, ComponentId::Session, 40);
    let wide = TextBuilder::tab_bar(&model, ComponentId::Session, 200);

    assert!(narrow.tabs.len() <= wide.tabs.len());
    assert!(!narrow.tabs.is_empty());
}

#[test]
fn every_tab_click_lands_inside_its_label() {
    let model = UiModel::default();
    let bar = TextBuilder::tab_bar(&model, ComponentId::Models, 120);

    for (start, end, _) in &bar.tabs {
        assert!(start < end);
    }
}

#[test]
fn the_status_line_never_overflows() {
    let mut model = UiModel::default();
    model.session_stats = Some(SessionStatsSummary {
        requests: 3,
        input_tokens: 12_000,
        output_tokens: 340,
        cache_read_tokens: 9_600,
        cache_write_tokens: 100,
        cost_usd: 0.001,
        context_used: 12_000,
        context_window: Some(200_000),
    });

    for columns in [40_u16, 80, 200] {
        let line = status_line(&model, ComponentId::Session, columns);
        assert!(width(&line) <= usize::from(columns));
    }
}

#[test]
fn the_status_names_the_queue_and_reasoning_modes() {
    let model = UiModel::default();
    let text = text_of(&status_line(&model, ComponentId::Session, 120));

    assert!(text.contains("queue steer"));
    assert!(text.contains("reasoning auto"));
}

#[test]
fn a_session_without_usage_shows_no_gauge() {
    let model = UiModel::default();
    let text = text_of(&status_line(&model, ComponentId::Session, 120));

    assert!(!text.contains('%'));
}

#[test]
fn the_tab_label_carries_its_digit() {
    assert_eq!(TextBuilder::view_label(ComponentId::Session), "Chat");
    assert!(ComponentId::Session.shortcut().is_some());
}

#[test]
fn the_tabs_stay_in_column_order() {
    let bar = TextBuilder::tab_bar(&UiModel::default(), ComponentId::Session, 30);
    let mut expected_start = 0;
    for (start, end, _) in &bar.tabs {
        assert!(*start >= expected_start);
        expected_start = *end;
    }
}

#[test]
fn the_theme_does_not_change_which_tabs_fit() {
    let mut light = UiModel::default();
    light.preferences.theme = ThemeMode::Light;
    let dark = TextBuilder::tab_bar(&UiModel::default(), ComponentId::Session, 120);
    let light = TextBuilder::tab_bar(&light, ComponentId::Session, 120);

    assert_eq!(dark.tabs.len(), light.tabs.len());
    for (a, b) in dark.tabs.iter().zip(light.tabs.iter()) {
        assert_eq!(a.2, b.2);
    }
}

#[test]
fn the_status_shows_the_context_gauge_when_the_host_reports_it() {
    let mut model = UiModel::default();
    model.session_stats = Some(SessionStatsSummary {
        requests: 4,
        input_tokens: 120_000,
        output_tokens: 900,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        cost_usd: 0.2,
        context_used: 120_000,
        context_window: Some(200_000),
    });

    let text = text_of(&status_line(&model, ComponentId::Session, 160));

    assert!(text.contains("60%"), "drawn: {text}");
    assert!(text.contains("120.0k/200.0k"), "drawn: {text}");
    assert!(text.contains("compacta 180.0k"), "drawn: {text}");
}

#[test]
fn a_full_context_turns_the_gauge_red() {
    let mut model = UiModel::default();
    model.session_stats = Some(SessionStatsSummary {
        context_used: 190_000,
        context_window: Some(200_000),
        ..SessionStatsSummary::default()
    });

    let line = status_line(&model, ComponentId::Session, 160);
    let colors = line
        .spans
        .iter()
        .filter(|span| span.content.contains('█'))
        .map(|span| span.style.fg)
        .collect::<Vec<_>>();

    assert!(
        colors
            .iter()
            .any(|color| *color == Some(crate::components::palette::danger())),
        "the filled cells are not danger colored"
    );
}
