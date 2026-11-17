use norte_code_wire::{InputPartKind, UserInputPart};

use super::Chip;

fn upload(kind: InputPartKind, path: &str) -> Chip {
    let part = UserInputPart::upload(
        kind,
        "upload-1".to_owned(),
        "application/octet-stream".to_owned(),
    );
    Chip::upload(part, path.to_owned(), kind)
}

#[test]
fn a_pasted_text_is_previewed_on_one_line() {
    let chip = Chip::text("first line\nsecond line".to_owned());

    assert_eq!(chip.to_string(), "[Copied: first line second line]");
}

#[test]
fn a_long_paste_is_truncated_with_an_ellipsis() {
    let chip = Chip::text("x".repeat(200));

    let shown = chip.to_string();
    assert!(shown.starts_with("[Copied: "));
    assert!(shown.ends_with("...]"));
}

#[test]
fn an_image_chip_names_its_kind_and_path() {
    let chip = upload(InputPartKind::Image, "~/pictures/a.png");

    assert_eq!(chip.to_string(), "[Image ~/pictures/a.png]");
}

#[test]
fn every_upload_kind_has_a_name() {
    let names = [
        (InputPartKind::Document, "a.pdf", "[File a.pdf]"),
        (InputPartKind::Audio, "a.wav", "[Audio a.wav]"),
        (InputPartKind::Video, "a.mp4", "[Video a.mp4]"),
    ];

    for (kind, path, expected) in names {
        assert_eq!(upload(kind, path).to_string(), expected);
    }
}
