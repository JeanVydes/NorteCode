//! Preference commands use the same values as the settings page.

use crate::diff::DiffMode;
use crate::theme::{ThemeChoice, ThemeKind};
use crate::ui::RailPosition;
use crate::ui_model::Preferences;

#[derive(Clone, Debug, PartialEq)]
pub(super) enum PreferenceCommand {
    Theme(String),
    Diff(DiffMode),
    Rail(RailPosition),
    ToolGrouping(Option<bool>),
    ToolOutput(Option<bool>),
    ChartDensity(u8),
    ChartContrast(u8),
    ChartDotSize(u8),
}

pub(super) fn parse(label: &str, args: &[&str]) -> Option<Result<PreferenceCommand, ()>> {
    let single = || match args {
        [value] => Ok(*value),
        _ => Err(()),
    };
    let toggle = || match args {
        [] => Ok(None),
        ["true"] => Ok(Some(true)),
        ["false"] => Ok(Some(false)),
        _ => Err(()),
    };
    let result = match label {
        "/theme" => {
            if args.is_empty() {
                Err(())
            } else {
                Ok(PreferenceCommand::Theme(args.join(" ")))
            }
        }
        "/diff" => single()
            .and_then(|value| DiffMode::from_label(value).ok_or(()))
            .map(PreferenceCommand::Diff),
        "/rail" => single()
            .and_then(|value| RailPosition::from_label(value).ok_or(()))
            .map(PreferenceCommand::Rail),
        "/tool-verbosity" | "/tool-grouping" => toggle().map(PreferenceCommand::ToolGrouping),
        "/tool-output-expand" => toggle().map(PreferenceCommand::ToolOutput),
        "/charts-density" => single()
            .and_then(|value| {
                value
                    .parse::<u8>()
                    .ok()
                    .filter(|value| (2..=6).contains(value))
                    .ok_or(())
            })
            .map(PreferenceCommand::ChartDensity),
        "/charts-contrast" => single()
            .and_then(|value| {
                (1..=16)
                    .find(|quarter| {
                        format!("{:.2}", f32::from(*quarter) / 4.0)
                            .trim_end_matches('0')
                            .trim_end_matches('.')
                            == value
                    })
                    .ok_or(())
            })
            .map(PreferenceCommand::ChartContrast),
        "/charts-dot-size" => single()
            .and_then(|value| match value {
                "small" => Ok(0),
                "medium" => Ok(1),
                "large" => Ok(2),
                _ => Err(()),
            })
            .map(PreferenceCommand::ChartDotSize),
        _ => return None,
    };
    Some(result)
}

pub(super) fn apply(command: &PreferenceCommand, preferences: &mut Preferences) -> Result<(), ()> {
    match command {
        PreferenceCommand::Theme(name) => {
            let built_in = [
                ThemeKind::Dark,
                ThemeKind::ByeRetina,
                ThemeKind::Bone,
                ThemeKind::Amoled,
            ]
            .into_iter()
            .find(|kind| kind.matches(name))
            .map(ThemeChoice::BuiltIn);
            let custom = preferences
                .catalog
                .custom
                .iter()
                .find(|theme| {
                    theme.name.eq_ignore_ascii_case(name)
                        || theme.name.replace(' ', "_").eq_ignore_ascii_case(name)
                })
                .map(|theme| ThemeChoice::Custom(theme.name.clone()));
            preferences.theme = built_in.or(custom).ok_or(())?;
        }
        PreferenceCommand::Diff(value) => preferences.diff_mode = *value,
        PreferenceCommand::Rail(value) => preferences.rail_position = *value,
        PreferenceCommand::ToolGrouping(value) => {
            preferences.tool_verbosity = value.unwrap_or(!preferences.tool_verbosity);
        }
        PreferenceCommand::ToolOutput(value) => {
            preferences.show_tool_output = value.unwrap_or(!preferences.show_tool_output);
        }
        PreferenceCommand::ChartDensity(value) => preferences.chart_density = *value,
        PreferenceCommand::ChartContrast(value) => preferences.chart_contrast = *value,
        PreferenceCommand::ChartDotSize(value) => preferences.chart_dot_size = *value,
    }
    Ok(())
}

pub(super) fn values(label: &str, preferences: &Preferences) -> Vec<String> {
    match label {
        "/theme" => [
            ThemeKind::Dark,
            ThemeKind::ByeRetina,
            ThemeKind::Bone,
            ThemeKind::Amoled,
        ]
        .into_iter()
        .map(|kind| kind.name().replace(' ', "_"))
        .chain(
            preferences
                .catalog
                .custom
                .iter()
                .map(|theme| theme.name.replace(' ', "_")),
        )
        .collect(),
        "/diff" => ["unified", "split"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        "/rail" => ["hide", "left", "right", "top", "bottom"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        "/tool-verbosity" | "/tool-grouping" | "/tool-output-expand" => {
            ["true", "false"].into_iter().map(str::to_owned).collect()
        }
        "/charts-density" => (2..=6).map(|value| value.to_string()).collect(),
        "/charts-contrast" => (1_u8..=16)
            .map(|value| {
                format!("{:.2}", f32::from(value) / 4.0)
                    .trim_end_matches('0')
                    .trim_end_matches('.')
                    .to_owned()
            })
            .collect(),
        "/charts-dot-size" => ["small", "medium", "large"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
#[path = "preferences_tests.rs"]
mod tests;
