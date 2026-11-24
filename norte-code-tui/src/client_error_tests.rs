//! Classification and display of connection failures.

use std::path::Path;

use super::ClientError;

#[test]
fn the_variant_survives_a_human_message() {
    let error = ClientError::send_prompt("offline");
    assert!(matches!(error, ClientError::SendPrompt(_)));
    assert_eq!(error.to_string(), "message refused: offline");
}

#[test]
fn attachment_failures_name_their_operation() {
    let read = ClientError::read_attachment(Path::new("/tmp/a.png"), "gone");
    assert!(matches!(read, ClientError::ReadAttachment(_)));
    assert_eq!(read.to_string(), "could not read /tmp/a.png: gone");

    let size = ClientError::attachment_too_large(9000, 8192);
    assert!(matches!(size, ClientError::AttachmentTooLarge(_)));
    assert_eq!(size.to_string(), "attachment has 9000 bytes, limit is 8192");
}

#[test]
fn key_save_failures_name_their_operation() {
    let error = ClientError::save_secret("read-only home");
    assert!(matches!(error, ClientError::SaveSecret(_)));
    assert_eq!(error.to_string(), "could not save the key: read-only home");
}

#[test]
fn key_delete_failures_name_their_operation() {
    let error = ClientError::delete_secret("read-only home");
    assert!(matches!(error, ClientError::DeleteSecret(_)));
    assert_eq!(
        error.to_string(),
        "could not delete the key: read-only home"
    );
}

#[test]
fn stream_failures_are_distinguished_from_command_failures() {
    assert!(matches!(
        ClientError::unstable_stream(),
        ClientError::UnstableStream
    ));
    assert!(matches!(
        ClientError::lost_stream("timeout"),
        ClientError::LostStream(_)
    ));
}
