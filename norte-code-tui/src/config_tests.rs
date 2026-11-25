//! Client configuration parsing behavior.

use std::path::Path;

use norte_code_client::config::{ReconnectConfig, RetentionConfig};

use super::{ClientConfig, ConfigError, UiConfig, parse_runtime};

const MINIMAL_CONFIG: &str = r#"{"service":{"url":"http://127.0.0.1:8787"}}"#;

#[test]
fn debug_defaults_off_and_can_be_enabled_in_json() {
    let plain: UiConfig = serde_json::from_str("{}").expect("empty UI config");
    assert!(!plain.debug);

    let enabled: UiConfig = serde_json::from_str(r#"{"debug":true}"#).expect("debug config");
    assert!(enabled.debug);
}

#[test]
fn tool_preferences_default_compact_and_load_from_ui_json() {
    let defaults: UiConfig = serde_json::from_str("{}").expect("empty UI config");
    assert!(!defaults.show_tool_output);
    assert!(!defaults.tool_verbosity);

    let configured: UiConfig =
        serde_json::from_str(r#"{"show_tool_output":true,"tool_verbosity":true}"#)
            .expect("tool preferences");
    assert!(configured.show_tool_output);
    assert!(configured.tool_verbosity);
}

#[test]
fn absent_runtime_blocks_keep_the_documented_defaults() {
    let config: ClientConfig = serde_json::from_str(MINIMAL_CONFIG).expect("minimal config");

    assert_eq!(
        config.reconnect.retry_base_ms,
        ReconnectConfig::default().retry_base_ms
    );
    assert_eq!(
        config.reconnect.retry_max_ms,
        ReconnectConfig::default().retry_max_ms
    );
    assert_eq!(
        config.reconnect.give_up_attempts,
        ReconnectConfig::default().give_up_attempts
    );
    assert_eq!(
        config.reconnect.healthy_stream_s,
        ReconnectConfig::default().healthy_stream_s
    );
    assert_eq!(
        config.retention.transcript_max_items,
        RetentionConfig::default().transcript_max_items
    );
    assert_eq!(
        config.retention.transcript_keep_items,
        RetentionConfig::default().transcript_keep_items
    );
    assert_eq!(
        config.retention.console_max_lines,
        RetentionConfig::default().console_max_lines
    );
    assert_eq!(
        config.retention.console_keep_lines,
        RetentionConfig::default().console_keep_lines
    );
}

#[test]
fn runtime_blocks_from_json_are_read_into_the_local_config() {
    let raw = r#"{
        "service": {"url": "http://127.0.0.1:8787"},
        "reconnect": {"retry_base_ms": 900, "retry_max_ms": 9000, "give_up_attempts": 7, "healthy_stream_s": 25},
        "retention": {"transcript_max_items": 120, "transcript_keep_items": 90, "console_max_lines": 60, "console_keep_lines": 45}
    }"#;
    let config: ClientConfig = serde_json::from_str(raw).expect("configured runtime values");

    assert_eq!(config.reconnect.retry_base_ms, 900);
    assert_eq!(config.reconnect.retry_max_ms, 9000);
    assert_eq!(config.reconnect.give_up_attempts, 7);
    assert_eq!(config.reconnect.healthy_stream_s, 25);
    assert_eq!(config.retention.transcript_max_items, 120);
    assert_eq!(config.retention.transcript_keep_items, 90);
    assert_eq!(config.retention.console_max_lines, 60);
    assert_eq!(config.retention.console_keep_lines, 45);
}

#[test]
fn out_of_range_runtime_values_are_rejected() {
    let raw = r#"{"reconnect":{"retry_base_ms":0}}"#;
    let error = parse_runtime(Path::new("cli.json"), raw).expect_err("zero base delay is invalid");
    assert!(matches!(error, ConfigError::InvalidRuntime { .. }));
    assert!(error.to_string().contains("retry_base_ms"));

    let raw = r#"{"retention":{"transcript_keep_items":2000}}"#;
    let error =
        parse_runtime(Path::new("cli.json"), raw).expect_err("a keep larger than max is invalid");
    assert!(matches!(error, ConfigError::InvalidRuntime { .. }));
}

#[test]
fn malformed_runtime_blocks_are_reported_as_json_errors() {
    let raw = r#"{"reconnect":"fast"}"#;
    let error = parse_runtime(Path::new("cli.json"), raw).expect_err("a string is not a block");
    assert!(matches!(error, ConfigError::Json { .. }));
}
