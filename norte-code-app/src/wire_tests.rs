use super::{InputPartKind, UserInputPart};

#[test]
fn uploaded_media_uses_raep_content_part_shape() {
    let parts = vec![UserInputPart::upload(
        InputPartKind::Audio,
        "upl_123".to_owned(),
        "audio/wav".to_owned(),
    )];
    assert_eq!(
        serde_json::to_value(parts).expect("typed parts serialize"),
        serde_json::json!([{
                "type": "audio",
                "source": {"kind": "upload", "upload_id": "upl_123"},
                "mime_type": "audio/wav"
        }])
    );
}
