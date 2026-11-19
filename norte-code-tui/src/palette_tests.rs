//! Color overrides: valid hex wins, invalid values fall back.

use super::{Palette, PaletteConfig};
use ratatui::style::Color;
use terminalui::Theme;

#[test]
fn configured_hex_overrides_the_base() {
    let config = PaletteConfig {
        background: Some("#0a0b0c".to_string()),
        accent: Some("FF00FF".to_string()),
        ..PaletteConfig::default()
    };
    let palette = Palette::from_config(config, &Palette::dark());
    assert_eq!(palette.background, Color::Rgb(10, 11, 12));
    assert_eq!(palette.accent, Color::Rgb(255, 0, 255));
    assert_eq!(palette.foreground, Color::Rgb(224, 227, 235));
}

#[test]
fn invalid_colors_keep_the_current_palette() {
    let config = PaletteConfig {
        background: Some("not a color".to_string()),
        border: Some("#12345".to_string()),
        ring: Some("#gg0000".to_string()),
        ..PaletteConfig::default()
    };
    let palette = Palette::from_config(config, &Palette::dark());
    assert_eq!(palette.background, Color::Rgb(22, 24, 29));
    assert_eq!(palette.border, Color::Rgb(58, 62, 74));
    assert_eq!(palette.ring, Color::Rgb(255, 47, 179));
}

#[test]
fn a_palette_becomes_the_render_theme() {
    let theme = Theme::from(Palette::amoled());
    assert_eq!(theme.background, Color::Rgb(0, 0, 0));
    assert_eq!(theme.accent, Color::Rgb(255, 47, 179));
}
