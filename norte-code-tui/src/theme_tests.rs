//! The theme catalog: built-ins stay, configured themes join the cycle.

use super::{CustomThemeConfig, ThemeCatalog, ThemeChoice, ThemeKind};
use crate::config::UiConfig;
use ratatui::style::Color;

fn config() -> UiConfig {
    serde_json::from_str(
        r##"{
            "theme": "ocean",
            "themes": [
                {
                    "name": "ocean",
                    "base": "dark",
                    "palette": { "accent": "#00aaff", "background": "101820" }
                },
                { "name": "solarized light", "base": "bye_retina" }
            ]
        }"##,
    )
    .expect("ui config")
}

fn catalog(config: &UiConfig) -> ThemeCatalog {
    ThemeCatalog {
        custom: config
            .themes
            .iter()
            .cloned()
            .map(super::CustomTheme::from_config)
            .collect(),
    }
}

#[test]
fn built_in_names_are_english() {
    assert_eq!(ThemeKind::Dark.name(), "Dark");
    assert_eq!(ThemeKind::ByeRetina.name(), "Bye Retina");
    assert_eq!(ThemeKind::Bone.name(), "Bone");
    assert_eq!(ThemeKind::Amoled.name(), "Amoled");
}

#[test]
fn built_in_themes_are_still_selectable() {
    let catalog = catalog(&config());
    assert_eq!(
        catalog.choose(Some("Bone")),
        ThemeChoice::BuiltIn(ThemeKind::Bone)
    );
    assert_eq!(
        catalog.choose(Some("Amoled")),
        ThemeChoice::BuiltIn(ThemeKind::Amoled)
    );
    assert_eq!(
        catalog.choose(Some("Bye Retina")),
        ThemeChoice::BuiltIn(ThemeKind::ByeRetina)
    );
    assert_eq!(
        catalog.choose(Some("bone")),
        ThemeChoice::BuiltIn(ThemeKind::Bone)
    );
}

#[test]
fn a_configured_theme_can_be_selected_by_name() {
    let catalog = catalog(&config());
    assert_eq!(
        catalog.choose(Some("ocean")),
        ThemeChoice::Custom("ocean".to_string())
    );
    assert_eq!(
        catalog.choose(Some("solarized_light")),
        ThemeChoice::Custom("solarized light".to_string())
    );
    assert_eq!(
        catalog.choose(Some("unknown")),
        ThemeChoice::BuiltIn(ThemeKind::Dark)
    );
}

#[test]
fn the_cycle_visits_builtins_and_each_configured_theme() {
    let catalog = catalog(&config());
    assert_eq!(
        catalog.next(&ThemeChoice::BuiltIn(ThemeKind::Amoled)),
        ThemeChoice::Custom("ocean".to_string())
    );
    assert_eq!(
        catalog.next(&ThemeChoice::Custom("ocean".to_string())),
        ThemeChoice::Custom("solarized light".to_string())
    );
    assert_eq!(
        catalog.next(&ThemeChoice::Custom("solarized light".to_string())),
        ThemeChoice::BuiltIn(ThemeKind::Dark)
    );
}

#[test]
fn a_configured_theme_applies_its_colors_over_its_base() {
    let catalog = catalog(&config());
    let choice = catalog.choose(Some("ocean"));
    let theme = catalog.theme(&choice);
    assert_eq!(theme.accent, Color::Rgb(0, 170, 255));
    assert_eq!(theme.background, Color::Rgb(16, 24, 32));
    // The rest comes from the dark built-in palette.
    assert_eq!(theme.muted, Color::Rgb(138, 145, 160));
}

#[test]
fn an_invalid_base_falls_back_to_dark() {
    let config: UiConfig =
        serde_json::from_str(r#"{ "themes": [{ "name": "plain", "base": "missing" }] }"#)
            .expect("ui config");
    let catalog = catalog(&config);
    let theme = catalog.theme(&catalog.choose(Some("plain")));
    assert_eq!(theme.background, Color::Rgb(22, 24, 29));
}

#[test]
fn a_configured_theme_without_colors_keeps_its_base() {
    let config = CustomThemeConfig {
        name: "copy".to_string(),
        base: Some(ThemeKind::ByeRetina),
        palette: crate::palette::PaletteConfig::default(),
    };
    let catalog = ThemeCatalog {
        custom: vec![super::CustomTheme::from_config(config)],
    };
    let theme = catalog.theme(&ThemeChoice::Custom("copy".to_string()));
    assert_eq!(theme.background, Color::Rgb(255, 255, 255));
}
