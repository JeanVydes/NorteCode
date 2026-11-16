use super::{CanonicalEvent, CatalogKind};

#[test]
fn the_tag_and_its_kind_decode_together() {
    let payload = serde_json::json!({"event": "agent_catalog", "kind": "plans", "entries": []});
    assert_eq!(
        CanonicalEvent::decode(&payload),
        Some(CanonicalEvent::AgentCatalog {
            kind: CatalogKind::Plans
        })
    );
    assert!(CanonicalEvent::plan_list().matches(&payload));
    assert!(!CanonicalEvent::agent_models().matches(&payload));
}

#[test]
fn a_tagged_event_ignores_the_rest_of_its_body() {
    let payload = serde_json::json!({"event": "token_delta", "turn": 3, "delta": "hi"});
    assert_eq!(
        CanonicalEvent::decode(&payload),
        Some(CanonicalEvent::TokenDelta)
    );
}

#[test]
fn a_tag_this_build_does_not_know_is_none() {
    let payload = serde_json::json!({"event": "something_new"});
    assert_eq!(CanonicalEvent::decode(&payload), None);
}
