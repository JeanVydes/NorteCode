use super::*;
use norte::domain::role::Role;

fn conversation() -> Conversation {
    Conversation {
        schema_version: CONVERSATION_SCHEMA_VERSION,
        session: SessionId::new(),
        phase: Phase::Idle,
        turn: 3,
        metadata: BTreeMap::from([("host".to_owned(), Value::String("x".to_owned()))]),
        options: None,
        messages: vec![
            Message::system("be terse"),
            Message::user("hi"),
            Message::assistant("hello"),
        ],
    }
}

#[test]
fn json_round_trip_is_byte_identical() {
    let conversation = conversation();
    let first = conversation.to_json().expect("serialize");
    let decoded = Conversation::from_json(&first).expect("deserialize");
    assert_eq!(decoded, conversation);
    let second = decoded.to_json().expect("serialize");
    assert_eq!(first, second);
}

#[test]
fn serialization_is_stable_across_runs() {
    let conversation = conversation();
    let first = conversation.to_json().expect("serialize");
    for _ in 0..8 {
        assert_eq!(conversation.to_json().expect("serialize"), first);
    }
}

#[test]
fn map_insertion_order_does_not_change_the_bytes() {
    let mut one = conversation();
    let mut two = conversation();
    two.session = one.session;
    one.metadata.clear();
    two.metadata.clear();
    for (key, value) in [("a", "1"), ("b", "2"), ("c", "3")] {
        one.metadata
            .insert(key.to_owned(), Value::String(value.to_owned()));
    }
    for (key, value) in [("c", "3"), ("a", "1"), ("b", "2")] {
        two.metadata
            .insert(key.to_owned(), Value::String(value.to_owned()));
    }
    assert_eq!(
        one.to_json().expect("serialize"),
        two.to_json().expect("serialize")
    );
}

#[test]
fn an_unknown_schema_version_is_rejected() {
    let json = serde_json::json!({
        "schema_version": 999,
        "session": SessionId::new(),
        "phase": "idle",
        "turn": 0,
        "messages": [],
    })
    .to_string();
    assert!(matches!(
        Conversation::from_json(&json),
        Err(norte::error::Error::SchemaVersion { .. })
    ));
}

#[test]
fn roles_survive_the_envelope() {
    let decoded =
        Conversation::from_json(&conversation().to_json().expect("serialize")).expect("decode");
    assert_eq!(decoded.messages[0].role, Role::System);
    assert_eq!(decoded.messages[1].role, Role::User);
    assert_eq!(decoded.messages[2].role, Role::Assistant);
}
