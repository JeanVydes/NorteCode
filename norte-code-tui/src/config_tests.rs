//! Client configuration parsing behavior.

use super::UiConfig;

#[test]
fn debug_defaults_off_and_can_be_enabled_in_json() {
    let plain: UiConfig = serde_json::from_str("{}").expect("empty UI config");
    assert!(!plain.debug);

    let enabled: UiConfig = serde_json::from_str(r#"{"debug":true}"#).expect("debug config");
    assert!(enabled.debug);
}
