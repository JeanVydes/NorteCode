use crate::UserInputAccepted;

#[test]
fn accepted_input_decodes_media_parts_echoed_by_the_host() {
    let payload = serde_json::json!({
        "turn_id": "turn-1",
        "origin": { "kind": "user" },
        "content": [
            { "type": "text", "text": "look at this" },
            {
                "type": "image",
                "source": { "kind": "upload", "upload_id": "upload-1" },
                "mime_type": "image/png"
            }
        ],
        "position": null,
        "replaces_turn_id": null
    });

    let accepted: UserInputAccepted =
        serde_json::from_value(payload).expect("accepted input decodes");

    assert_eq!(accepted.content.len(), 2);
    assert_eq!(accepted.turn_id, "turn-1");
}
