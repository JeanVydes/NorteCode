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

use crate::runtime::{ConsultationPoint, Interception, InterceptionContext, TurnInterceptor};
use base64::Engine as _;
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
#[path = "screenshots_tests.rs"]
mod tests;
