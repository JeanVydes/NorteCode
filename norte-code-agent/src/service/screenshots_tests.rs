use super::*;
use crate::runtime::{EngineEvent, EventSink};
use norte::{InMemoryBlobStore, Role};

/// A one-pixel PNG: small, real, and enough to prove the bytes survive.
///
/// The same bytes `tests.rs` writes to a file for the paste path, so a
/// capture here goes through the same decoder that path does.
fn one_pixel_png() -> Vec<u8> {
    vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ]
}

/// A sink that drops every engine event, which is all these tests need.
struct SilentSink;

impl EventSink for SilentSink {
    fn emit(&self, _event: EngineEvent) {}
}

fn png_base64() -> String {
    base64::engine::general_purpose::STANDARD.encode(one_pixel_png())
}

fn output_with_capture(capture: &str) -> ToolOutput {
    ToolOutput {
        parts: vec![Part::text("screenshot: 100 bytes of base64 PNG")],
        structured: Some(serde_json::json!({
            "message": "Screenshot captured successfully",
            "screenshot_base64": capture,
        })),
        is_error: false,
    }
}

fn publisher(screenshots: &Arc<Screenshots>) -> Publisher {
    Publisher {
        screenshots: Arc::clone(screenshots),
        blobs: Arc::new(InMemoryBlobStore::new()),
        preprocessor: preprocessor(&norte_code_protocol::config::ScreenshotConfig::default()),
    }
}

#[tokio::test]
async fn a_capture_leaves_the_result_and_waits_as_a_blob() {
    // What this file exists for: the PNG stops being dead base64 inside the
    // tool result and becomes a blob the interceptor can attach. The field
    // is removed, so the result that stays in the history forever no longer
    // carries the image.
    let screenshots = Arc::new(Screenshots::default());
    let session = SessionId::new();
    let mut output = output_with_capture(&png_base64());
    publisher(&screenshots).publish(session, &mut output).await;

    let structured = output.structured.as_ref().expect("structured");
    assert!(
        structured.get(SCREENSHOT_FIELD).is_none(),
        "the dead copy is gone: {structured}"
    );
    assert_eq!(
        structured["message"], "Screenshot captured successfully",
        "and the rest of the answer is untouched"
    );
    // The text the model reads is unchanged: the answer still says that a
    // screenshot was taken and how large it was.
    let text: String = output.parts.iter().filter_map(Part::as_text).collect();
    assert!(text.contains("100 bytes of base64 PNG"), "{text}");

    let queued = screenshots.take(session);
    assert_eq!(queued.len(), 1, "one capture is waiting");
    assert_eq!(queued[0].kind, MediaKind::Image);
    assert!(matches!(queued[0].source, MediaSource::Blob(_)));
    assert_eq!(queued[0].mime.as_str(), "image/png");
}

#[tokio::test]
async fn a_result_that_is_not_a_capture_is_left_alone() {
    // Every other browser answer: elements, tabs, a navigate. Nothing is
    // queued, so no image is invented for an action that took none.
    let screenshots = Arc::new(Screenshots::default());
    let session = SessionId::new();
    let mut output = ToolOutput {
        parts: vec![Part::text("2 tabs open")],
        structured: Some(serde_json::json!({ "message": "2 tabs open" })),
        is_error: false,
    };
    publisher(&screenshots).publish(session, &mut output).await;
    assert!(
        screenshots.take(session).is_empty(),
        "a plain answer queues no capture"
    );
}

#[tokio::test]
async fn a_capture_that_is_not_base64_is_not_queued() {
    // A tool that answered with something else under the key is not a
    // capture. Nothing is queued and the field stays where it is, so the
    // answer is unchanged rather than silently emptied.
    let screenshots = Arc::new(Screenshots::default());
    let session = SessionId::new();
    let mut output = output_with_capture("not base64 at all!!");
    publisher(&screenshots).publish(session, &mut output).await;
    assert!(
        screenshots.take(session).is_empty(),
        "an unreadable capture queues nothing"
    );
    assert!(
        output
            .structured
            .as_ref()
            .is_some_and(|s| s.get(SCREENSHOT_FIELD).is_some()),
        "the field is untouched when nothing was queued"
    );
}

#[test]
fn a_capture_is_bounded_by_the_configured_size() {
    // The bound is the host's and it reaches the preprocessor: a capture
    // larger than the configured longest side is downscaled rather than
    // sent whole. Without this the number in the config is decoration.
    let small = preprocessor(&norte_code_protocol::config::ScreenshotConfig {
        enabled: true,
        max_dimension: 32,
        max_bytes: 1024,
    });
    assert_eq!(small.max_dimension, 32);
    assert_eq!(small.max_bytes, 1024);
    // PNG, because a screenshot is mostly text and JPEG artifacts land on
    // the strokes a model has to read.
    assert_eq!(small.output, ImageOutput::Png);
}

#[tokio::test]
async fn the_interceptor_injects_one_user_message_carrying_the_image() {
    // The end of the chain: a queued capture becomes a user message whose
    // parts are the introduction and the image, which is the shape every
    // wire can carry and a tool result cannot (on `chat`, at least).
    let screenshots = Arc::new(Screenshots::default());
    let session = SessionId::new();
    let mut output = output_with_capture(&png_base64());
    let blobs = Arc::new(InMemoryBlobStore::new());
    let publisher = Publisher {
        screenshots: Arc::clone(&screenshots),
        blobs: Arc::clone(&blobs) as Arc<dyn BlobStore>,
        preprocessor: preprocessor(&norte_code_protocol::config::ScreenshotConfig::default()),
    };
    publisher.publish(session, &mut output).await;

    let with_image = Capabilities::builder()
        .input(Modality::Text)
        .input(Modality::Image)
        .build();
    let interceptor = ScreenshotInterceptor::new(Arc::clone(&screenshots), &with_image);
    let sink = SilentSink;
    let context = InterceptionContext {
        point: ConsultationPoint::PreflightRequest,
        session,
        turn: 1,
        head: None,
        messages: &[],
        events: &sink,
    };
    let Interception::Inject(messages) = interceptor.intercept(&context).await.expect("injects")
    else {
        panic!("a capture the model can take is injected");
    };
    assert_eq!(messages.len(), 1, "one message, not one per part");
    assert_eq!(messages[0].role, Role::User);
    assert_eq!(
        messages[0]
            .parts
            .iter()
            .filter_map(Part::as_text)
            .collect::<String>(),
        INTRODUCTION,
        "the model is told what the image is"
    );
    let media = messages[0]
        .parts
        .iter()
        .find_map(Part::as_media)
        .expect("the image is a part of the same message");
    assert_eq!(media.kind, MediaKind::Image);
    assert_eq!(media.mime.as_str(), "image/png");

    // And the bytes are the capture's: the blob the part points at holds
    // what the tool produced, so the provider gets the real screenshot.
    let MediaSource::Blob(id) = media.source else {
        panic!("the part points at a blob");
    };
    assert_eq!(
        blobs.get(id).await.expect("stored"),
        one_pixel_png(),
        "the stored bytes are the capture, unchanged"
    );

    // Taken once: a second call has nothing to inject, so the image is not
    // attached twice.
    assert!(matches!(
        interceptor.intercept(&context).await.expect("continues"),
        Interception::Continue
    ));
}

#[tokio::test]
async fn a_capture_for_a_text_only_model_is_dropped_not_queued() {
    // The safeguard. An interceptor's messages are committed before the
    // engine checks them against the model, so an image injected into a
    // text-only model would sit in the history and fail every later turn.
    // It is dropped instead, and the queue is emptied so it cannot surface
    // on a later turn either.
    let screenshots = Arc::new(Screenshots::default());
    let session = SessionId::new();
    let mut output = output_with_capture(&png_base64());
    publisher(&screenshots).publish(session, &mut output).await;

    let text_only = Capabilities::builder().input(Modality::Text).build();
    let interceptor = ScreenshotInterceptor::new(Arc::clone(&screenshots), &text_only);
    let sink = SilentSink;
    let context = InterceptionContext {
        point: ConsultationPoint::PreflightRequest,
        session,
        turn: 1,
        head: None,
        messages: &[],
        events: &sink,
    };
    assert!(matches!(
        interceptor.intercept(&context).await.expect("continues"),
        Interception::Continue
    ));
    assert!(
        screenshots.take(session).is_empty(),
        "and nothing is left waiting for a turn that could take it"
    );
}

#[test]
fn the_interceptor_knows_whether_the_model_takes_an_image() {
    // The check that keeps a capture out of a text-only model's history. An
    // interceptor's messages are committed before the engine checks them
    // against the model, so injecting one it cannot take would fail every
    // later turn: the way a refused attachment used to poison a session.
    let screenshots = Arc::new(Screenshots::default());
    let text_only = Capabilities::builder().input(Modality::Text).build();
    let interceptor = ScreenshotInterceptor::new(Arc::clone(&screenshots), &text_only);
    assert!(!interceptor.supports_image);

    let with_image = Capabilities::builder()
        .input(Modality::Text)
        .input(Modality::Image)
        .build();
    let interceptor = ScreenshotInterceptor::new(screenshots, &with_image);
    assert!(interceptor.supports_image);
}
