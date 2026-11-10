use super::SseDecoder;

#[test]
fn decodes_complete_event_after_split_chunks() {
    let mut decoder = SseDecoder::default();
    decoder.push(b"event: TOKEN_DELTA\ndata: {\"delta\":");
    assert_eq!(decoder.next_data().expect("valid partial frame"), None);
    decoder.push(b"\"hello\"}\n\n");
    assert_eq!(
        decoder.next_data().expect("complete frame"),
        Some("{\"delta\":\"hello\"}".to_owned())
    );
}

#[test]
fn supports_crlf_and_ignores_comment_events() {
    let mut decoder = SseDecoder::default();
    decoder.push(b": keepalive\r\n\r\ndata: {\"value\":1}\r\n\r\n");
    assert_eq!(
        decoder.next_data().expect("complete frame"),
        Some("{\"value\":1}".to_owned())
    );
}
