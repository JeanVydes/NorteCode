use super::{apply, parse, values};
use crate::ui_model::UiModel;

#[test]
fn preference_commands_validate_apply_and_offer_supported_values() {
    let mut preferences = UiModel::detached().preferences;
    for (command, argument) in [
        ("/theme", "Amoled"),
        ("/diff", "split"),
        ("/rail", "right"),
        ("/tool-grouping", "true"),
        ("/tool-output-expand", "true"),
        ("/charts-density", "6"),
        ("/charts-contrast", "2.75"),
        ("/charts-dot-size", "large"),
    ] {
        assert!(
            values(command, &preferences)
                .iter()
                .any(|value| value == argument)
        );
        let preference = parse(command, &[argument]).unwrap().unwrap();
        apply(&preference, &mut preferences).unwrap();
    }
    assert_eq!(
        preferences.theme,
        crate::theme::ThemeChoice::BuiltIn(crate::theme::ThemeKind::Amoled)
    );
    assert_eq!(preferences.diff_mode, crate::diff::DiffMode::Split);
    assert_eq!(preferences.rail_position, crate::ui::RailPosition::Right);
    assert!(preferences.tool_verbosity);
    assert!(preferences.show_tool_output);
    assert_eq!(preferences.chart_density, 6);
    assert_eq!(preferences.chart_contrast, 11);
    assert_eq!(preferences.chart_dot_size, 2);
    apply(
        &parse("/tool-grouping", &[]).unwrap().unwrap(),
        &mut preferences,
    )
    .unwrap();
    apply(
        &parse("/tool-output-expand", &[]).unwrap().unwrap(),
        &mut preferences,
    )
    .unwrap();
    assert!(!preferences.tool_verbosity && !preferences.show_tool_output);
    assert!(parse("/charts-density", &["7"]).unwrap().is_err());
    assert!(parse("/charts-contrast", &["1.1"]).unwrap().is_err());
    assert!(parse("/tool-grouping", &["sometimes"]).unwrap().is_err());
    assert!(
        apply(
            &parse("/theme", &["unknown"]).unwrap().unwrap(),
            &mut preferences
        )
        .is_err()
    );
}
