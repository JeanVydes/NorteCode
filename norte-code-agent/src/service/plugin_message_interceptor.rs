use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use norte::domain::part::Part;
use norte::{
    BlobStore, CallId, Capabilities, MediaKind, MediaSource, Message, Modality, Result, Role,
};
use norte_code_wire::{PluginId, PluginMessageRejection};

use crate::runtime::{
    ConsultationPoint, EngineEvent, Interception, InterceptionContext, TurnInterceptor,
};

use super::plugin_message_provenance::PluginMessageProvenance;
use super::plugin_messages::{PendingPluginMessage, PluginMessages};

#[derive(Clone, Debug, Default)]
struct SupportedModalities(BTreeSet<Modality>);

impl SupportedModalities {
    fn new(capabilities: &Capabilities) -> Self {
        Self(capabilities.modalities.input.keys().copied().collect())
    }

    fn accepts(&self, part: &Part) -> bool {
        let modality = match part {
            Part::Text(_) => Modality::Text,
            Part::Media(media) => match media.kind {
                MediaKind::Image => Modality::Image,
                MediaKind::Audio => Modality::Audio,
                MediaKind::Video => Modality::Video,
                MediaKind::Document => Modality::Document,
                _ => return false,
            },
            _ => return false,
        };
        self.0.contains(&modality)
    }
}

/// Commits accepted plugin drafts as user messages after the tools ran.
pub(super) struct PluginMessageInterceptor {
    messages: Arc<PluginMessages>,
    blobs: Arc<dyn BlobStore>,
    max_messages_per_turn: usize,
    supported: SupportedModalities,
}

impl PluginMessageInterceptor {
    pub(super) fn new(
        messages: Arc<PluginMessages>,
        blobs: Arc<dyn BlobStore>,
        config: &norte_code_protocol::config::PluginMessagesConfig,
        capabilities: &Capabilities,
    ) -> Self {
        Self {
            messages,
            blobs,
            max_messages_per_turn: config.max_messages_per_turn,
            supported: SupportedModalities::new(capabilities),
        }
    }

    async fn adapt(&self, pending: PendingPluginMessage) -> Result<Message> {
        let parts = self.store_media(pending.parts).await?;
        let Some(provenance) = PluginMessageProvenance::part(&pending.plugin_id, pending.call_id)
        else {
            return Err(norte::Error::InvalidInput(
                "a plugin message could not carry its provenance".to_owned(),
            ));
        };
        let mut message = Message::new(Role::User, parts);
        message.parts.push(provenance);
        Ok(message)
    }

    fn report(
        context: &InterceptionContext<'_>,
        plugin_id: PluginId,
        call_id: CallId,
        reason: PluginMessageRejection,
    ) {
        context.events.emit(EngineEvent::PluginMessageRejected {
            session: context.session,
            plugin_id,
            call_id,
            reason,
        });
    }

    async fn store_media(&self, parts: Vec<Part>) -> Result<Vec<Part>> {
        let mut stored = Vec::with_capacity(parts.len());
        for mut part in parts {
            let Part::Media(media) = &mut part else {
                stored.push(part);
                continue;
            };
            let blob = match &media.source {
                MediaSource::Inline(bytes) => Some(self.blobs.put(bytes).await?),
                _ => None,
            };
            if let Some(id) = blob {
                media.source = MediaSource::Blob(id);
            }
            stored.push(part);
        }
        Ok(stored)
    }
}

#[async_trait]
impl TurnInterceptor for PluginMessageInterceptor {
    fn name(&self) -> &'static str {
        "plugin_messages"
    }

    async fn intercept(&self, context: &InterceptionContext<'_>) -> Result<Interception> {
        if context.point != ConsultationPoint::PostTools {
            return Ok(Interception::Continue);
        }
        let waiting = self.messages.take(context.session);
        if waiting.pending.is_empty() && waiting.rejected.is_empty() {
            return Ok(Interception::Continue);
        }
        for rejection in waiting.rejected {
            Self::report(
                context,
                rejection.plugin_id,
                rejection.call_id,
                rejection.reason,
            );
        }
        let mut injected = Vec::new();
        for pending in waiting.pending {
            if injected.len() >= self.max_messages_per_turn {
                Self::report(
                    context,
                    pending.plugin_id,
                    pending.call_id,
                    PluginMessageRejection::LimitReached,
                );
                continue;
            }
            if !pending
                .parts
                .iter()
                .all(|part| self.supported.accepts(part))
            {
                Self::report(
                    context,
                    pending.plugin_id,
                    pending.call_id,
                    PluginMessageRejection::UnsupportedModality,
                );
                continue;
            }
            let plugin_id = pending.plugin_id.clone();
            let call_id = pending.call_id;
            let message = self.adapt(pending).await?;
            let parts = message.parts.len();
            context.events.emit(EngineEvent::PluginMessageInjected {
                session: context.session,
                plugin_id,
                call_id,
                parts,
            });
            injected.push(message);
        }
        if injected.is_empty() {
            Ok(Interception::Continue)
        } else {
            Ok(Interception::Inject(injected))
        }
    }
}
