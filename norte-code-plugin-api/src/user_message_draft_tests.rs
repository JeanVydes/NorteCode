use crate::UserMessageDraft;
use norte::domain::part::{ExtensionPart, Part};
use norte::{Error, MediaKind, MediaPart, MediaSource, MimeType};

#[test]
fn accepts_text_and_media() {
    let draft = UserMessageDraft::text("look").expect("draft");
    assert_eq!(draft.parts().len(), 1);
    assert_eq!(draft.parts()[0].as_text(), Some("look"));
}

#[test]
fn accepts_media() {
    let mime = MimeType::new("image/png").expect("mime");
    let media = Part::Media(MediaPart::new(
        MediaKind::Image,
        MediaSource::Inline(vec![1, 2, 3]),
        mime,
    ));
    let draft = UserMessageDraft::new(vec![media]).expect("draft");
    assert_eq!(draft.parts().len(), 1);
}

#[test]
fn refuses_empty_text() {
    assert!(matches!(
        UserMessageDraft::text("   "),
        Err(Error::InvalidInput(_))
    ));
}

#[test]
fn refuses_parts_that_are_not_text_or_media() {
    let result = UserMessageDraft::new(vec![Part::Extension(ExtensionPart {
        namespace: "plugin".to_owned(),
        data: serde_json::Value::Null,
    })]);
    assert!(matches!(result, Err(Error::InvalidInput(_))));
}
