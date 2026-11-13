use std::path::Path;

use super::{input_part_kind, mime_of_path, modality_of, modality_word};
use norte_code_wire::InputPartKind;

#[test]
fn an_image_travels_as_an_image() {
    assert_eq!(input_part_kind("image/png"), Some(InputPartKind::Image));
    assert_eq!(
        modality_of(InputPartKind::Image),
        norte_code_wire::OutputModality::Image
    );
}

#[test]
fn a_document_travels_as_a_document() {
    assert_eq!(
        input_part_kind("application/pdf"),
        Some(InputPartKind::Document)
    );
    assert_eq!(
        modality_of(InputPartKind::Document),
        norte_code_wire::OutputModality::Document
    );
}

#[test]
fn a_media_type_no_client_sends_has_no_part() {
    assert_eq!(input_part_kind("application/x-executable"), None);
    assert_eq!(input_part_kind(""), None);
}

#[test]
fn a_modality_has_a_word_a_person_reads() {
    assert_eq!(
        modality_word(norte_code_wire::OutputModality::Image),
        "image"
    );
    assert_eq!(
        modality_word(norte_code_wire::OutputModality::Document),
        "document"
    );
}

#[test]
fn a_file_name_names_its_media_type() {
    assert_eq!(mime_of_path(Path::new("photo.png")), Some("image/png"));
    assert_eq!(
        mime_of_path(Path::new("notes.PDF")),
        Some("application/pdf")
    );
    assert_eq!(mime_of_path(Path::new("a/b/song.mp3")), Some("audio/mpeg"));
}

#[test]
fn a_file_name_without_a_known_type_has_no_media_type() {
    assert_eq!(mime_of_path(Path::new("program")), None);
    assert_eq!(mime_of_path(Path::new("archive.unknownext")), None);
}
