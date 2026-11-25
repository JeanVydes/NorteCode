use super::*;

#[test]
fn creating_a_session_with_a_model_serializes_the_provider_override() {
    let body = CreateSessionBody {
        agent_id: "norte-code".to_owned(),
        title: None,
        agent_override: Some(CreateAgentOverride {
            model: CreateAgentModel {
                id: "sonnet".to_owned(),
            },
            provider: "anthropic".to_owned(),
        }),
    };
    let value = serde_json::to_value(body).expect("serialize create request");
    assert_eq!(value["agent_override"]["provider"], "anthropic");
    assert_eq!(value["agent_override"]["model"]["id"], "sonnet");
}

#[test]
fn creating_a_session_without_a_model_omits_the_override() {
    let body = CreateSessionBody {
        agent_id: "norte-code".to_owned(),
        title: None,
        agent_override: None,
    };
    let value = serde_json::to_value(body).expect("serialize create request");
    assert!(value.get("agent_override").is_none());
}
