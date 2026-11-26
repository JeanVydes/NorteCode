//! Chip labels and draft behavior.

use norte_code_wire::UploadReceipt;

use super::{ChipLabel, Draft, OriginalText};
use crate::attachment::AttachmentKind;

#[test]
fn removing_a_pending_image_removes_its_attachment() {
    let mut draft = Draft::default();
    draft.insert_pending(crate::attachment::PreparedAttachment {
        kind: AttachmentKind::Image,
        mime: crate::attachment::MimeType::png(),
        source: crate::attachment::AttachmentSource::Image(vec![1, 2, 3]),
        cursor: 0,
    });
    assert_eq!(draft.text(), "[Image #1]");
    assert_eq!(draft.pending_attachments().len(), 1);
    draft.replace_range(0, draft.len(), "");
    assert_eq!(draft.pending_attachments().len(), 0);
}

fn receipt() -> UploadReceipt {
    UploadReceipt {
        upload_id: "upl_1".to_owned(),
        mime_type: "image/png".to_owned(),
        bytes: 12,
        sha256: "digest".to_owned(),
        expires_at: 0,
    }
}

#[test]
fn long_paste_label_previews_six_visible_characters() {
    let label = ChipLabel::copied(&OriginalText::new("Lorem ipsum dolor sit amet".to_owned()));
    assert_eq!(label.as_str(), "[Copied: Lorem i...]");
}

#[test]
fn upload_labels_number_each_kind_on_its_own() {
    let mut draft = Draft::default();
    draft.insert_upload(0, AttachmentKind::Image, receipt());
    draft.insert_upload(draft.len(), AttachmentKind::File, receipt());
    draft.insert_upload(draft.len(), AttachmentKind::Image, receipt());
    assert_eq!(draft.text(), "[Image #1][File #1][Image #2]");
}

#[test]
fn a_chip_is_removed_as_one_piece() {
    let mut draft = Draft::default();
    draft.replace_range(0, 0, "before ");
    draft.insert_text_chip(7, OriginalText::new("some long pasted text".to_owned()));
    draft.replace_range(draft.len(), draft.len(), " after");
    let start = draft.spans()[0].start;
    let (start, end) = draft.chip_range_at(start + 2).expect("chip under caret");
    let cursor = draft.replace_range(start, end, "");
    assert_eq!(draft.text(), "before  after");
    assert_eq!(cursor, 7);
}

#[test]
fn editing_inside_a_label_replaces_the_whole_chip() {
    let mut draft = Draft::default();
    draft.insert_text_chip(0, OriginalText::new("a long pasted value".to_owned()));
    let inside = draft.spans()[0].start + 3;
    draft.replace_range(inside, inside + 1, "x");
    assert_eq!(draft.text(), "x");
    assert_eq!(draft.spans(), []);
}

#[test]
fn expanded_text_uses_original_content_and_real_markers() {
    let mut draft = Draft::default();
    draft.insert_text_chip(0, OriginalText::new("the original long text".to_owned()));
    draft.replace_range(draft.len(), draft.len(), " and typed");
    draft.insert_upload(draft.len(), AttachmentKind::File, receipt());
    assert_eq!(
        draft.expanded_text(),
        "the original long text and typed[File #1]"
    );
}

#[test]
fn wire_parts_only_carry_uploads() {
    let mut draft = Draft::default();
    draft.insert_text_chip(0, OriginalText::new("the original long text".to_owned()));
    draft.insert_upload(draft.len(), AttachmentKind::Image, receipt());
    let parts = draft.wire_parts();
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0].mime_type.as_deref(), Some("image/png"));
}
