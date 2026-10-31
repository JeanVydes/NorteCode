//! Browser screenshots, promoted to something the model can look at.
//!
//! `browser_use` answers a `screenshot` with the PNG as base64 inside its
//! result's `structured` value, and the model reads the text of that result:
//! `screenshot: 326 624 bytes of base64 PNG`. The image itself never reached it,
//! and on the chat completions wire it cannot — a tool message carries text only
//! there, and the adapter refuses media in a tool result with a typed capability
//! error rather than dropping it silently (`norte-openai/src/chat/request.rs`).
//!
//! Every wire *does* have a shape for an image in a user message. So the capture
//! is taken out of the tool result, bounded, stored as a blob, and injected as a
//! user message carrying one `Part::Media` — which the engine commits, so it
//! lands in the history, in the request digest and in what the policies read,
//! exactly as a pasted image does.
//!
//! Two seams, and each is the only one that fits:
//!
//! * The **executor wrapper** takes the capture. It is the only place that sees
//!   a tool's output together with the session it belongs to, and it sees it
//!   once. Walking the history for the field instead would re-promote every
//!   capture ever taken on every later turn, because a tool result stays in the
//!   history forever.
//! * The **interceptor** injects it, because the engine consults it at the safe
//!   point just after the tools ran and commits what it returns. A layer cannot:
//!   a layer sees the request after it was built, so what it added would be
//!   neither durable nor visible to the policies.
//!
//! The interceptor checks the model's own modalities before injecting, and
//! injects nothing when the model cannot take an image. That check is not
//! optional: an interceptor's messages are committed *before* the engine checks
//! them against the model, so a capture injected into a text-only model would
//! land in the history and fail every later turn — the exact way a refused
//! attachment used to poison a session.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use base64::Engine as _;
use norte::harness::{ConsultationPoint, Interception, InterceptionContext, TurnInterceptor};
use norte::media::{ImageOutput, ImagePreprocessor};
use norte::{
    BlobStore, Capabilities, FnExecutor, MediaKind, MediaPart, MediaPayload, MediaPreprocessor,
    MediaSource, Message, MimeType, Modality, Part, Result, SessionId, ToolArgs, ToolContext,
    ToolDeclaration, ToolOutput,
};

/// The key `browser_use` writes its capture under, in the result's `structured`.
const SCREENSHOT_FIELD: &str = "screenshot_base64";

/// What the tool captures.
const SCREENSHOT_MIME: &str = "image/png";

/// The line that introduces the image to the model.
///
/// The message is the host's, not the human's, and it says so: an image
/// appearing unexplained after a tool result leaves the model to guess whether
/// a person sent it.
const INTRODUCTION: &str = "The screenshot from `browser_use` is attached.";

/// The captures waiting for the engine to take them, one queue per session.
///
/// The queue is what carries the bytes between the executor, which runs while
/// the tools do, and the interceptor, which runs when the next request is about
/// to be built.
#[derive(Debug, Default)]
pub(super) struct Screenshots {
    pending: Mutex<BTreeMap<SessionId, VecDeque<MediaPart>>>,
}

impl Screenshots {
    /// Queues a capture for the session.
    fn push(&self, session: SessionId, media: MediaPart) {
        self.lock().entry(session).or_default().push_back(media);
    }

    /// Takes every capture the session has waiting, oldest first.
    fn take(&self, session: SessionId) -> Vec<MediaPart> {
        self.lock().remove(&session).unwrap_or_default().into()
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<SessionId, VecDeque<MediaPart>>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Bounds a capture, stores it, and queues it — removing it from the tool's
/// answer on the way through.
///
/// The bytes are re-encoded to fit the configured bounds, so what reaches the
/// provider is an image that fits a context window rather than a full-page PNG.
/// `ImagePreprocessor` does the work; it is idempotent and leaves an image
/// already within both bounds untouched.
///
/// The field is removed from the result once the capture is safely queued.
/// Leaving it would store the same PNG twice: once as the blob the model reads
/// and once as base64 in the history that nothing reads — 3.5 MB of it in a
/// measured session, since the tool result is rebuilt on every later turn. The
/// text the model reads still says how big the capture was, so the answer stays
/// honest about what happened; only the dead copy goes.
struct Publisher {
    screenshots: Arc<Screenshots>,
    blobs: Arc<dyn BlobStore>,
    preprocessor: ImagePreprocessor,
}

impl Publisher {
    /// Takes the capture out of `output`, or leaves the output alone.
    async fn publish(&self, session: SessionId, output: &mut ToolOutput) {
        let Some(capture) = output
            .structured
            .as_ref()
            .and_then(|structured| structured.get(SCREENSHOT_FIELD))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
        else {
            return;
        };
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&capture) else {
            tracing::warn!(session = %session, "a browser screenshot was not valid base64");
            return;
        };
        let Ok(mime) = MimeType::new(SCREENSHOT_MIME) else {
            return;
        };
        let bounded = match self
            .preprocessor
            .preprocess(MediaPayload::new(MediaKind::Image, mime, bytes))
            .await
        {
            Ok(bounded) => bounded,
            Err(error) => {
                // A capture that cannot be decoded is not sent, and the field
                // stays where it is: an image nobody can read is not an
                // improvement on the number the model already has.
                tracing::warn!(session = %session, %error, "a browser screenshot was not usable");
                return;
            }
        };
        let Ok(id) = self.blobs.put(&bounded.bytes).await else {
            tracing::warn!(session = %session, "a browser screenshot could not be stored");
            return;
        };
        self.screenshots.push(
            session,
            MediaPart::new(MediaKind::Image, MediaSource::Blob(id), bounded.mime),
        );
        if let Some(object) = output
            .structured
            .as_mut()
            .and_then(serde_json::Value::as_object_mut)
        {
            object.remove(SCREENSHOT_FIELD);
        }
    }
}

/// Wraps a tool so its output is read for a capture before the model sees it.
struct PublisherExecutor<A> {
    inner: Arc<dyn FnExecutor<A, ToolOutput>>,
    publisher: Arc<Publisher>,
}

#[async_trait::async_trait]
impl<A> FnExecutor<A, ToolOutput> for PublisherExecutor<A>
where
    A: ToolArgs,
{
    async fn call(&self, args: A) -> Result<ToolOutput> {
        self.inner.call(args).await
    }

    async fn call_with_context(&self, context: &ToolContext, args: A) -> Result<ToolOutput> {
        let mut output = self.inner.call_with_context(context, args).await?;
        // Only the context-aware path is wrapped: the engine always calls this
        // one, and the capture needs the session to be queued against.
        if let Some(session) = context.session {
            self.publisher.publish(session, &mut output).await;
        }
        Ok(output)
    }
}

/// Wraps a tool declaration so a capture in its output reaches the queue.
///
/// Any tool whose result carries the field is published; in practice only
/// `browser_use` writes it, which is why the host wraps that one declaration.
pub(super) fn publishing_tool<A>(
    declaration: ToolDeclaration<A>,
    screenshots: Arc<Screenshots>,
    blobs: Arc<dyn BlobStore>,
    preprocessor: ImagePreprocessor,
) -> ToolDeclaration<A>
where
    A: ToolArgs + serde::de::DeserializeOwned + std::fmt::Debug + 'static,
{
    ToolDeclaration {
        name: declaration.name,
        description: declaration.description,
        parameters: declaration.parameters,
        executor: Arc::new(PublisherExecutor {
            inner: declaration.executor,
            publisher: Arc::new(Publisher {
                screenshots,
                blobs,
                preprocessor,
            }),
        }),
    }
}

/// The interceptor that promotes a queued capture into a user message.
pub(super) struct ScreenshotInterceptor {
    screenshots: Arc<Screenshots>,
    /// Whether the model takes an image at all, from its own capabilities.
    supports_image: bool,
}

impl ScreenshotInterceptor {
    /// Builds an interceptor for a model with the given capabilities.
    pub(super) fn new(screenshots: Arc<Screenshots>, capabilities: &Capabilities) -> Self {
        Self {
            screenshots,
            supports_image: capabilities.modalities.supports_input(Modality::Image),
        }
    }
}

#[async_trait::async_trait]
impl TurnInterceptor for ScreenshotInterceptor {
    fn name(&self) -> &'static str {
        "screenshots"
    }

    async fn intercept(&self, context: &InterceptionContext<'_>) -> Result<Interception> {
        if context.point != ConsultationPoint::PreflightRequest {
            return Ok(Interception::Continue);
        }
        let captures = self.screenshots.take(context.session);
        if captures.is_empty() {
            return Ok(Interception::Continue);
        }
        // Taken and not returned: a capture the model cannot take is dropped
        // rather than queued for a turn that could take it, because the next
        // turn of this session is about to run on this same model, and a stale
        // screenshot attached to it would be worse than none.
        if !self.supports_image {
            tracing::warn!(
                session = %context.session,
                captures = captures.len(),
                "browser screenshots were not sent: the model does not take image input"
            );
            return Ok(Interception::Continue);
        }
        let messages = captures
            .into_iter()
            .map(|media| {
                let mut message = Message::user(INTRODUCTION);
                message.parts.push(Part::Media(media));
                message
            })
            .collect();
        Ok(Interception::Inject(messages))
    }
}

/// The preprocessor a capture is bounded by, from the host's configuration.
#[must_use]
pub(super) fn preprocessor(
    config: &norte_code_protocol::config::ScreenshotConfig,
) -> ImagePreprocessor {
    // PNG and not the default JPEG: a screenshot is mostly text, and JPEG
    // artifacts land exactly on the strokes a model has to read.
    let bytes = usize::try_from(config.max_bytes).unwrap_or(usize::MAX);
    ImagePreprocessor::new(config.max_dimension, bytes, ImageOutput::Png, 100)
}

#[cfg(test)]
mod tests {
    use super::*;
    use norte::harness::{EngineEvent, EventSink};
    use norte::{InMemoryBlobStore, Role};

    /// A one-pixel PNG: small, real, and enough to prove the bytes survive.
    ///
    /// The same bytes `tests.rs` writes to a file for the paste path, so a
    /// capture here goes through the same decoder that path does.
    fn one_pixel_png() -> Vec<u8> {
        vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
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
        let Interception::Inject(messages) =
            interceptor.intercept(&context).await.expect("injects")
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
        // later turn — the way a refused attachment used to poison a session.
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
}
