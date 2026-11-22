use crate::{HOST_EFFECTS_KEY, HostEffects, UserMessageDraft};
use norte::ToolOutput;
use serde_json::{Map, Value};

fn structured_output() -> ToolOutput {
    let mut fields = Map::new();
    fields.insert("ok".to_owned(), Value::Bool(true));
    ToolOutput::json(Value::Object(fields))
}

#[test]
fn attach_and_extract_round_trip() {
    let effects = HostEffects::user_message(UserMessageDraft::text("look").expect("draft"));
    let mut output = effects.attach(structured_output()).expect("attach");
    assert!(
        output
            .structured
            .as_ref()
            .and_then(Value::as_object)
            .is_some_and(|fields| fields.contains_key(HOST_EFFECTS_KEY))
    );

    let extracted = HostEffects::extract(&mut output)
        .expect("extract")
        .expect("effects");
    assert_eq!(extracted.user_messages.len(), 1);
    assert!(
        output
            .structured
            .as_ref()
            .and_then(Value::as_object)
            .is_some_and(|fields| !fields.contains_key(HOST_EFFECTS_KEY))
    );
}

#[test]
fn extract_without_effects_leaves_the_result_alone() {
    let mut output = structured_output();
    assert!(
        HostEffects::extract(&mut output)
            .expect("extract")
            .is_none()
    );
    assert_eq!(
        output.structured,
        Some(Value::Object(Map::from_iter([(
            "ok".to_owned(),
            Value::Bool(true)
        )])))
    );
}

#[test]
fn attach_refuses_a_result_without_structured_data() {
    let effects = HostEffects::user_message(UserMessageDraft::text("look").expect("draft"));
    assert!(effects.attach(ToolOutput::text("done")).is_err());
}
