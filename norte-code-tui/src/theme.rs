//! Theme selection: the built-ins plus themes declared in the UI config.
//!
//! A theme names a palette. Built-ins have fixed palettes; configured themes
//! overlay their colors on a built-in base. Nothing here paints: the
//! resolved `Palette` becomes the library's render theme.

use serde::Deserialize;
use terminalui::Theme;

use crate::palette::{Palette, PaletteConfig};

/// Which built-in theme is active.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ThemeKind {
    /// Dark background, the default.
    Dark,
    /// Blinding white, maximum contrast.
    Cegador,
    /// Warm bone pastel.
    PastelHueso,
    /// True black, #000, for OLED panels.
    Amoled,
}

impl ThemeKind {
    /// The display name used by the status chrome.
    pub fn name(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Cegador => "cegador",
            Self::PastelHueso => "pastel hueso",
            Self::Amoled => "amoled",
        }
    }

    /// The next theme in the cycle order.
    pub fn next(self) -> Self {
        match self {
            Self::Dark => Self::Cegador,
            Self::Cegador => Self::PastelHueso,
            Self::PastelHueso => Self::Amoled,
            Self::Amoled => Self::Dark,
        }
    }

    fn palette(self) -> Palette {
        match self {
            Self::Dark => Palette::dark(),
            Self::Cegador => Palette::cegador(),
            Self::PastelHueso => Palette::pastel_hueso(),
            Self::Amoled => Palette::amoled(),
        }
    }
}

/// A configured theme: a name and color overrides on a built-in base.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct CustomThemeConfig {
    /// The name the status chrome shows and `ui.theme` selects.
    pub name: String,
    /// Which built-in palette to start from; `dark` by default.
    #[serde(default, deserialize_with = "deserialize_base")]
    pub base: Option<ThemeKind>,
    /// Color overrides; missing roles keep the base color.
    #[serde(default)]
    pub palette: PaletteConfig,
}

/// A configured theme with its palette.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CustomTheme {
    /// The name the status chrome shows.
    pub name: String,
    /// The palette this theme resolves to.
    pub palette: Palette,
}

impl CustomTheme {
    /// Resolve the configured colors over the base palette.
    pub fn from_config(config: CustomThemeConfig) -> Self {
        let base = config.base.unwrap_or(ThemeKind::Dark);
        Self {
            name: config.name,
            palette: Palette::from_config(config.palette, &base.palette()),
        }
    }

    fn matches(&self, name: &str) -> bool {
        self.name.eq_ignore_ascii_case(name)
            || self.name.replace(' ', "_").eq_ignore_ascii_case(name)
    }
}

/// Every theme the interface can paint with.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ThemeCatalog {
    /// Themes declared in the UI config, in cycle order.
    pub custom: Vec<CustomTheme>,
}

impl ThemeCatalog {
    /// Select a theme by name; built-ins keep their names.
    pub fn choose(&self, name: Option<&str>) -> ThemeChoice {
        let Some(name) = name else {
            return ThemeChoice::BuiltIn(ThemeKind::Dark);
        };
        for kind in [
            ThemeKind::Dark,
            ThemeKind::Cegador,
            ThemeKind::PastelHueso,
            ThemeKind::Amoled,
        ] {
            if kind.name().eq_ignore_ascii_case(name)
                || kind.name().replace(' ', "_").eq_ignore_ascii_case(name)
            {
                return ThemeChoice::BuiltIn(kind);
            }
        }
        if let Some(theme) = self.custom.iter().find(|theme| theme.matches(name)) {
            return ThemeChoice::Custom(theme.name.clone());
        }
        ThemeChoice::BuiltIn(ThemeKind::Dark)
    }

    /// The next theme in cycle order: built-ins first, then configured ones.
    pub fn next(&self, choice: &ThemeChoice) -> ThemeChoice {
        match choice {
            ThemeChoice::BuiltIn(kind) => {
                let next = kind.next();
                if next == ThemeKind::Dark && !self.custom.is_empty() {
                    return ThemeChoice::Custom(self.custom[0].name.clone());
                }
                ThemeChoice::BuiltIn(next)
            }
            ThemeChoice::Custom(name) => {
                let position = self.custom.iter().position(|theme| &theme.name == name);
                match position {
                    Some(index) if index + 1 < self.custom.len() => {
                        ThemeChoice::Custom(self.custom[index + 1].name.clone())
                    }
                    _ => ThemeChoice::BuiltIn(ThemeKind::Dark),
                }
            }
        }
    }

    /// Resolve a selection to the palette the interface paints with.
    pub fn palette(&self, choice: &ThemeChoice) -> Palette {
        match choice {
            ThemeChoice::BuiltIn(kind) => kind.palette(),
            ThemeChoice::Custom(name) => self
                .custom
                .iter()
                .find(|theme| &theme.name == name)
                .map_or_else(|| ThemeKind::Dark.palette(), |theme| theme.palette.clone()),
        }
    }

    /// Resolve a selection to the library's render theme.
    pub fn theme(&self, choice: &ThemeChoice) -> Theme {
        self.palette(choice).into()
    }
}

/// The name shown in the status chrome.
pub fn theme_name(choice: &ThemeChoice) -> &str {
    match choice {
        ThemeChoice::BuiltIn(kind) => kind.name(),
        ThemeChoice::Custom(name) => name,
    }
}

/// A built-in theme or one declared in the UI config.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ThemeChoice {
    /// One of the four built-ins.
    BuiltIn(ThemeKind),
    /// A theme declared by the user.
    Custom(String),
}

fn deserialize_base<'de, D>(deserializer: D) -> Result<Option<ThemeKind>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let name = Option::<String>::deserialize(deserializer)?;
    Ok(name.and_then(|name| match name.as_str() {
        "dark" => Some(ThemeKind::Dark),
        "cegador" => Some(ThemeKind::Cegador),
        "pastel_hueso" | "pastel hueso" => Some(ThemeKind::PastelHueso),
        "amoled" => Some(ThemeKind::Amoled),
        _ => None,
    }))
}

#[cfg(test)]
#[path = "theme_tests.rs"]
mod tests;
