#![allow(clippy::unwrap_used)]

use super::*;

#[test]
fn subagent_defaults_to_the_parent_agent() {
    let loaded = Config::from_json("{}").expect("empty config");
    assert_eq!(loaded.config.default_subagent_delegation, None);
}

#[test]
fn subagent_agent_round_trips() {
    let loaded =
        Config::from_json(r#"{"default_subagent_delegation":"research"}"#).expect("config");
    assert_eq!(
        loaded.config.default_subagent_delegation.as_deref(),
        Some("research")
    );
}

#[test]
fn service_rejects_an_unknown_subagent_agent() {
    let service = ServiceConfig {
        default_agent: "main".to_owned(),
        agents: vec![Agent {
            name: "main".to_owned(),
            default_subagent_delegation: Some("missing".to_owned()),
            ..Agent::default()
        }],
        ..ServiceConfig::default()
    };
    let error = service.validate().expect_err("unknown agent");
    assert!(
        error
            .to_string()
            .contains("does not name a configured agent")
    );
}

#[test]
fn checked_in_service_template_is_valid() {
    let service: ServiceConfig = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../default/service.json"
    )))
    .expect("service template");
    service.validate().expect("valid service template");
}
