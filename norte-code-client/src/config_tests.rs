use super::*;

#[test]
fn defaults_match_the_old_client_behavior() {
    let config = CliRuntimeConfig::default();
    config.validate().expect("defaults");
    assert_eq!(config.service_lifecycle.tokio_workers, 4);
    assert_eq!(config.service_lifecycle.startup_wait_ms, 20_000);
    assert_eq!(config.reconnect.retry_base_ms, 400);
    assert_eq!(config.reconnect.retry_max_ms, 5_000);
    assert_eq!(config.retention.transcript_max_items, 1_000);
    assert_eq!(config.retention.console_keep_lines, 400);
}

#[test]
fn a_partial_runtime_block_keeps_defaults() {
    let config: CliRuntimeConfig =
        serde_json::from_str(r#"{"reconnect":{"retry_base_ms":1000}}"#).expect("config");
    assert_eq!(config.reconnect.retry_base_ms, 1_000);
    assert_eq!(config.reconnect.retry_max_ms, 5_000);
    assert_eq!(config.service_lifecycle.stop_wait_ms, 10_000);
}

#[test]
fn an_inverted_retention_is_refused() {
    let config: CliRuntimeConfig =
        serde_json::from_str(r#"{"retention":{"transcript_keep_items":1100}}"#).expect("config");
    let error = config.validate().expect_err("invalid retention");
    assert_eq!(error.field, "retention");
}

#[test]
fn a_reconnect_ceiling_below_the_base_is_refused() {
    let config: CliRuntimeConfig =
        serde_json::from_str(r#"{"reconnect":{"retry_max_ms":100}}"#).expect("config");
    let error = config.validate().expect_err("invalid backoff");
    assert_eq!(error.field, "reconnect.retry_max_ms");
}

#[test]
fn checked_in_cli_template_supplies_the_runtime_blocks() {
    let raw = include_str!("../../default/cli.json");
    let config: CliRuntimeConfig = serde_json::from_str(raw).expect("cli template");
    config.validate().expect("cli template limits");
    assert_eq!(config.service_lifecycle.tokio_workers, 4);
    assert_eq!(config.reconnect.retry_base_ms, 400);
    assert_eq!(config.retention.transcript_keep_items, 800);
    assert_eq!(config.retention.console_keep_lines, 400);
}
