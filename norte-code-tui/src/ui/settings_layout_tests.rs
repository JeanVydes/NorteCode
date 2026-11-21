//! Settings sections stay separated and their rows map to the right lists.

use super::{SettingsLayout, SettingsSection};

fn layout() -> SettingsLayout {
    SettingsLayout::new(100, 4, 3, 2, 6)
}

#[test]
fn sections_follow_the_display_order_without_overlapping() {
    let layout = layout();
    let mut previous_end = 0;
    for section in SettingsSection::ALL {
        let rect = layout.section(section);
        assert!(rect.y >= previous_end);
        assert!(rect.height > 0);
        previous_end = rect.y + rect.height;
    }
}

#[test]
fn preferences_stay_after_plugins_and_before_agents() {
    let layout = layout();
    let plugins = layout.section(SettingsSection::Plugins).y;
    let preferences = layout.section(SettingsSection::Preferences).y;
    let agents = layout.section(SettingsSection::Agents).y;
    assert!(plugins < preferences);
    assert!(preferences < agents);
}

#[test]
fn body_rows_start_after_their_heading_and_header() {
    let layout = layout();
    let prefs = layout.section(SettingsSection::Preferences);
    let plugins = layout.section(SettingsSection::Plugins);

    assert_eq!(layout.preference_row(prefs.y), None);
    assert_eq!(layout.preference_row(prefs.y + 1), Some(0));
    assert_eq!(layout.plugin_row(plugins.y + 2), None);
    assert_eq!(layout.plugin_row(plugins.y + 3), Some(0));
    assert_eq!(layout.plugin_row(plugins.y + 5), Some(2));
}

#[test]
fn preference_columns_stay_inside_the_content_width() {
    let layout = SettingsLayout::new(48, 0, 0, 0, 6);
    assert!(layout.value_x() <= 48 - SettingsLayout::MARGIN);
    assert!(layout.label_width() > 0);
}
