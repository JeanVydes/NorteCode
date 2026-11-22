use std::sync::{Arc, Mutex};

use norte::domain::part::Part;
use norte::{
    Capabilities, InMemoryBlobStore, MediaKind, MediaPart, MediaSource, MimeType, Modality, Role,
};
use norte_code_plugin_api::{HostEffects, UserMessageDraft};
use norte_code_protocol::config::{Config, PluginMessagesConfig, PluginPolicyConfig};
use norte_code_wire::{PluginId, PluginMessageRejection};

use crate::runtime::{
    ConsultationPoint, EngineEvent, EventSink, Interception, InterceptionContext, TurnInterceptor,
};

use super::plugin_message_interceptor::PluginMessageInterceptor;
use super::plugin_message_provenance::PluginMessageProvenance;
use super::plugin_messages::{PluginMessagePolicy, PluginMessages};

fn policy(allow_media: bool) -> PluginMessagePolicy {
    let mut config = Config::default();
    config.tools.plugin_messages = PluginMessagesConfig {
        enabled: true,
        max_messages_per_turn: 2,
        max_bytes: 4096,
        allow_media,
    };
    config.plugins.insert(
        PluginId::new("browser"),
        PluginPolicyConfig {
            allow_user_messages: true,
        },
    );
    PluginMessagePolicy::new(&config)
}

fn draft(text: &str) -> UserMessageDraft {
    UserMessageDraft::text(text).expect("draft")
}

#[derive(Default)]
struct RecordingSink(Mutex<Vec<EngineEvent>>);

impl EventSink for RecordingSink {
    fn emit(&self, event: EngineEvent) {
        self.0.lock().expect("sink").push(event);
    }
}

#[test]
fn a_call_effect_is_queued_once() {
    let messages = PluginMessages::default();
    let session = norte::SessionId::new();
    let call = norte::CallId::new();
    let plugin = PluginId::new("browser");
    let effects = HostEffects::user_message(draft("look"));
    let policy = policy(false);

    messages.enqueue(session, &plugin, call, vec![effects.clone()], &policy);
    messages.enqueue(session, &plugin, call, vec![effects], &policy);

    assert_eq!(messages.take(session).pending.len(), 1);
    assert_eq!(messages.take(session).pending.len(), 0);
}

#[test]
fn a_plugin_without_permission_is_not_queued() {
    let messages = PluginMessages::default();
    let session = norte::SessionId::new();
    let effects = HostEffects::user_message(draft("look"));
    let mut config = Config::default();
    config.tools.plugin_messages.enabled = true;

    messages.enqueue(
        session,
        &PluginId::new("browser"),
        norte::CallId::new(),
        vec![effects],
        &PluginMessagePolicy::new(&config),
    );

    let waiting = messages.take(session);
    assert_eq!(waiting.pending.len(), 0);
    assert_eq!(waiting.rejected.len(), 1);
    assert_eq!(
        waiting.rejected[0].reason,
        PluginMessageRejection::PluginNotAllowed
    );
}

#[test]
fn a_disabled_global_switch_drops_every_message() {
    let messages = PluginMessages::default();
    let session = norte::SessionId::new();
    let effects = HostEffects::user_message(draft("look"));
    let mut config = Config::default();
    config.plugins.insert(
        PluginId::new("browser"),
        PluginPolicyConfig {
            allow_user_messages: true,
        },
    );

    messages.enqueue(
        session,
        &PluginId::new("browser"),
        norte::CallId::new(),
        vec![effects],
        &PluginMessagePolicy::new(&config),
    );

    let waiting = messages.take(session);
    assert_eq!(waiting.pending.len(), 0);
    assert_eq!(waiting.rejected.len(), 1);
    assert_eq!(waiting.rejected[0].reason, PluginMessageRejection::Disabled);
}

#[test]
fn an_oversized_message_is_dropped() {
    let messages = PluginMessages::default();
    let session = norte::SessionId::new();
    let mut config = Config::default();
    config.tools.plugin_messages.enabled = true;
    config.tools.plugin_messages.max_bytes = 2;
    config.plugins.insert(
        PluginId::new("browser"),
        PluginPolicyConfig {
            allow_user_messages: true,
        },
    );

    messages.enqueue(
        session,
        &PluginId::new("browser"),
        norte::CallId::new(),
        vec![HostEffects::user_message(draft("too long"))],
        &PluginMessagePolicy::new(&config),
    );

    let waiting = messages.take(session);
    assert_eq!(waiting.pending.len(), 0);
    assert_eq!(waiting.rejected.len(), 1);
    assert_eq!(waiting.rejected[0].reason, PluginMessageRejection::TooLarge);
}

#[test]
fn media_is_rejected_when_the_switch_is_off() {
    let messages = PluginMessages::default();
    let session = norte::SessionId::new();
    let mime = MimeType::new("image/png").expect("mime");
    let media = Part::Media(MediaPart::new(
        MediaKind::Image,
        MediaSource::Inline(vec![1, 2, 3]),
        mime,
    ));
    let draft = UserMessageDraft::new(vec![media]).expect("draft");

    messages.enqueue(
        session,
        &PluginId::new("browser"),
        norte::CallId::new(),
        vec![HostEffects::user_message(draft)],
        &policy(false),
    );

    let waiting = messages.take(session);
    assert_eq!(waiting.pending.len(), 0);
    assert_eq!(waiting.rejected.len(), 1);
    assert_eq!(
        waiting.rejected[0].reason,
        PluginMessageRejection::MediaNotAllowed
    );
}

#[tokio::test]
async fn an_effect_becomes_a_user_message_with_provenance() {
    let messages = Arc::new(PluginMessages::default());
    let session = norte::SessionId::new();
    let call = norte::CallId::new();
    let plugin = PluginId::new("browser");
    messages.enqueue(
        session,
        &plugin,
        call,
        vec![HostEffects::user_message(draft("look"))],
        &policy(false),
    );
    let blobs = Arc::new(InMemoryBlobStore::new());
    let capabilities = Capabilities::builder().input(Modality::Text).build();
    let interceptor = PluginMessageInterceptor::new(
        Arc::clone(&messages),
        blobs,
        &norte_code_protocol::config::PluginMessagesConfig {
            enabled: true,
            max_messages_per_turn: 2,
            max_bytes: 4096,
            allow_media: false,
        },
        &capabilities,
    );
    let sink = RecordingSink::default();
    let context = InterceptionContext {
        point: ConsultationPoint::PostTools,
        session,
        turn: 1,
        head: None,
        messages: &[],
        events: &sink,
    };

    let interception = interceptor.intercept(&context).await.expect("intercept");
    let Interception::Inject(injected) = interception else {
        panic!("expected an injected message");
    };
    assert_eq!(injected.len(), 1);
    assert_eq!(injected[0].role, Role::User);
    let extension = injected[0]
        .parts
        .iter()
        .find_map(|part| match part {
            Part::Extension(extension) => Some(extension),
            _ => None,
        })
        .expect("provenance");
    let provenance: PluginMessageProvenance =
        serde_json::from_value(extension.data.clone()).expect("provenance data");
    assert_eq!(provenance.plugin_id, plugin);
    assert_eq!(provenance.call_id, call.to_string());
    assert_eq!(sink.0.lock().expect("sink").len(), 1);
}

#[test]
fn provenance_is_read_back_from_the_persisted_message() {
    let plugin = PluginId::new("browser");
    let call = norte::CallId::new();
    let message = norte::Message::new(
        Role::User,
        vec![
            Part::text("from plugin"),
            PluginMessageProvenance::part(&plugin, call).expect("provenance part"),
        ],
    );

    let value = PluginMessageProvenance::from_message(&message).expect("provenance");
    let provenance: PluginMessageProvenance =
        serde_json::from_value(value).expect("provenance data");
    assert_eq!(provenance.plugin_id, plugin);
    assert_eq!(provenance.call_id, call.to_string());
}

#[tokio::test]
async fn a_text_only_model_does_not_receive_media() {
    let messages = Arc::new(PluginMessages::default());
    let session = norte::SessionId::new();
    let mime = MimeType::new("image/png").expect("mime");
    let media = Part::Media(MediaPart::new(
        MediaKind::Image,
        MediaSource::Inline(vec![1, 2, 3]),
        mime,
    ));
    let draft = UserMessageDraft::new(vec![media]).expect("draft");
    messages.enqueue(
        session,
        &PluginId::new("browser"),
        norte::CallId::new(),
        vec![HostEffects::user_message(draft)],
        &policy(true),
    );
    let blobs = Arc::new(InMemoryBlobStore::new());
    let capabilities = Capabilities::builder().input(Modality::Text).build();
    let interceptor = PluginMessageInterceptor::new(
        Arc::clone(&messages),
        blobs,
        &norte_code_protocol::config::PluginMessagesConfig {
            enabled: true,
            max_messages_per_turn: 2,
            max_bytes: 4096,
            allow_media: true,
        },
        &capabilities,
    );
    let sink = RecordingSink::default();
    let context = InterceptionContext {
        point: ConsultationPoint::PostTools,
        session,
        turn: 1,
        head: None,
        messages: &[],
        events: &sink,
    };

    assert_eq!(
        interceptor.intercept(&context).await.expect("intercept"),
        Interception::Continue
    );
    let events = sink.0.lock().expect("sink");
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        EngineEvent::PluginMessageRejected {
            reason: PluginMessageRejection::UnsupportedModality,
            ..
        }
    ));
}

#[tokio::test]
async fn the_second_message_in_a_turn_is_reported_when_the_limit_is_reached() {
    let messages = Arc::new(PluginMessages::default());
    let session = norte::SessionId::new();
    let plugin = PluginId::new("browser");
    for text in ["first", "second"] {
        messages.enqueue(
            session,
            &plugin,
            norte::CallId::new(),
            vec![HostEffects::user_message(draft(text))],
            &policy(false),
        );
    }
    let blobs = Arc::new(InMemoryBlobStore::new());
    let capabilities = Capabilities::builder().input(Modality::Text).build();
    let interceptor = PluginMessageInterceptor::new(
        Arc::clone(&messages),
        blobs,
        &norte_code_protocol::config::PluginMessagesConfig {
            enabled: true,
            max_messages_per_turn: 1,
            max_bytes: 4096,
            allow_media: false,
        },
        &capabilities,
    );
    let sink = RecordingSink::default();
    let context = InterceptionContext {
        point: ConsultationPoint::PostTools,
        session,
        turn: 1,
        head: None,
        messages: &[],
        events: &sink,
    };

    let interception = interceptor.intercept(&context).await.expect("intercept");
    let Interception::Inject(injected) = interception else {
        panic!("expected one injected message");
    };
    assert_eq!(injected.len(), 1);
    let events = sink.0.lock().expect("sink");
    assert_eq!(events.len(), 2);
    assert!(events.iter().any(|event| matches!(
        event,
        EngineEvent::PluginMessageRejected {
            reason: PluginMessageRejection::LimitReached,
            ..
        }
    )));
}
