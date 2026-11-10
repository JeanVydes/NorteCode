use super::*;
use norte::{Part, ToolOutput};

/// A shell result as the tool renders it, for the code the caller names.
fn shell_result(code: &str) -> ToolOutput {
    let mut output = ToolOutput::text("the text the model sees");
    output.structured = Some(serde_json::json!({ "code": code }));
    output
}

#[test]
fn only_a_detached_shell_result_reads_as_detached() {
    // The one place the wire's `detached` comes from. The tool renders every
    // outcome into the same structured value, so a code that is merely
    // present is not enough: reading the wrong one would draw a timeout or a
    // cancelled call as a detach, and the client would stop reporting the
    // endings a person acts on.
    assert!(is_detached(&shell_result("detached")));

    for code in ["success", "non_zero_exit", "timeout", "cancelled"] {
        assert!(
            !is_detached(&shell_result(code)),
            "`{code}` is not a detach"
        );
    }

    // A result with no structured value at all: nothing to read.
    assert!(!is_detached(&ToolOutput::text("plain")));
    // And one whose code is not a string.
    let mut odd = ToolOutput::text("plain");
    odd.structured = Some(serde_json::json!({ "code": 7 }));
    assert!(!is_detached(&odd));
}

#[test]
fn a_result_carries_the_text_the_model_saw() {
    // The helper reads `parts` first and falls back to the structured value,
    // which is what makes the distinction above possible: the text of a
    // detached call is a sentence, and its code is a field.
    let output = shell_result("detached");
    assert_eq!(
        output.parts.iter().filter_map(Part::as_text).count(),
        1,
        "the text is what the model reads"
    );
}

#[test]
fn a_mime_type_names_the_kind_the_part_is_built_with() {
    // The check and the part have to agree, and they did not: the part was
    // built as `Image` whatever the file was, so a PDF was checked against
    // the model as a document — the modality a `pdf` override names — and
    // then sent as an image. A model that takes documents and not images
    // would have passed the check and failed at the provider.
    assert_eq!(kind_of_mime("image/png"), MediaKind::Image);
    assert_eq!(kind_of_mime("image/jpeg"), MediaKind::Image);
    assert_eq!(kind_of_mime("audio/wav"), MediaKind::Audio);
    assert_eq!(kind_of_mime("video/mp4"), MediaKind::Video);
    assert_eq!(kind_of_mime("application/pdf"), MediaKind::Document);
    // A type with no subtype, and one with none at all: the whole string is
    // read, and anything not image, audio or video is a document.
    assert_eq!(kind_of_mime("application"), MediaKind::Document);
    assert_eq!(kind_of_mime(""), MediaKind::Document);
}
