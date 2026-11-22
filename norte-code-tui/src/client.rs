//! The client layer: how the interface talks to the raep server.
//!
//! One task owns the connection; attaching, streaming and sending all
//! happen there. Commands come in on one channel, events go out on
//! another. The behavior is deliberately stricter than a best-effort loop:
//! a failed attach reports its reason and stops, waiting for an explicit
//! retry; a stream that keeps dying gives up after a few attempts instead
//! of retrying forever.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use norte_code_client::config::ReconnectConfig;
use norte_code_client::{AgentClient, EventStream, RemoteClient, RestClient};
use norte_code_wire::UserInputPart;
use norte_code_wire::{SessionKey, SessionSummary, UploadReceipt};
use tokio::sync::mpsc::{self, Receiver, Sender};

use crate::answer::QuestionAnswer;
use crate::attachment::{AttachmentSource, PreparedAttachment};
use crate::client_error::ClientError;
use crate::config::ClientConfig;
use crate::shared_telemetry::SharedTelemetry;

/// Events the connection reports to the interface.
#[derive(Clone, Debug)]
pub(crate) enum ClientEvent {
    /// The attach completed and the stream is open.
    Attached {
        /// The session the interface is now attached to.
        session: norte_code_wire::SessionSummary,
    },
    /// The connection is down; contains the reason.
    Detached {
        /// What went wrong, in words a person can act on.
        reason: ClientError,
    },
    /// A server event arrived.
    Server(Box<norte_code_wire::ServerEvent>),
    /// The session list changed.
    Sessions(Vec<SessionSummary>),
    /// The catalogue of models, grouped by provider.
    Models(Vec<norte_code_wire::ModelInfo>),
    /// The plan list of the attached session.
    Plans(Vec<norte_code_wire::PlanSummary>),
    /// The job list of the attached session.
    Jobs(Vec<norte_code_wire::JobSummary>),
    /// Telemetry for the whole fleet.
    Telemetry(SharedTelemetry),
    /// The cost of the attached session.
    CostSession(norte_code_client::CostReport),
    /// The fleet-wide cost total.
    CostGlobal(norte_code_client::CostReport),
    /// The list of installed plugins.
    Plugins(Vec<norte_code_client::PluginSummary>),
    /// The catalogue detail (providers, models) for the HOST view.
    Catalog(norte_code_client::CatalogDetail),
    /// The server-reported agents.
    Agents(Vec<norte_code_wire::AgentSummary>),
    /// What the server says about itself.
    ServerInfo(norte_code_client::ServerInfo),
    /// The liveness probe answer.
    Liveness(norte_code_client::Liveness),
    /// The readiness probe answer.
    Readiness(norte_code_client::Readiness),
    /// The tenant's quota report.
    Quota(norte_code_wire::QuotaReport),
    /// A fork completed; the host should open this session.
    Forked {
        /// The new session, as listed by the host.
        session: Box<norte_code_wire::SessionSummary>,
    },
    /// A command failed; carry the reason for the transcript.
    SendFailed {
        /// What went wrong, in words a person can act on.
        reason: ClientError,
    },
    /// The full history of the attached session, newest first.
    History(Vec<norte_code_wire::HistoryItem>),
    /// Something the host did, worth telling the reader.
    Notice {
        /// The words.
        text: String,
    },
    /// The session's history could not be fetched; carry the reason.
    HistoryFailed {
        /// The transport or server error.
        reason: ClientError,
    },
    /// The host stored the bytes; the composer can add its chip.
    Uploaded {
        /// The attachment the chip belongs to.
        attachment: PreparedAttachment,
        /// The host's receipt.
        receipt: UploadReceipt,
    },
    /// The bytes never reached the host.
    UploadFailed {
        /// What went wrong, in words a person can act on.
        reason: ClientError,
    },
    /// The host stored a provider key.
    SecretSaved {
        /// The provider the key belongs to.
        provider: String,
    },
    /// The host refused to store a provider key.
    SecretSaveFailed {
        /// What went wrong, in words a person can act on.
        reason: ClientError,
    },
}

/// Commands the interface sends to the connection task.
#[derive(Clone, Debug)]
pub(crate) enum UiCommand {
    /// Send a user message on the session's stream.
    Send {
        /// The session to send on.
        session: SessionKey,
        /// The text of the message.
        text: String,
        /// Uploaded parts that travel with the message.
        parts: Vec<UserInputPart>,
        /// `steer` or `queue`; forwarded to the host.
        queue_mode: String,
    },
    /// Store an attachment's bytes before it can be sent.
    Upload {
        /// The session that will own the bytes.
        session: SessionKey,
        /// The attachment to upload.
        attachment: PreparedAttachment,
    },
    /// Refetch the session list.
    Refresh,
    /// Create a fresh session and attach to it.
    CreateSession {
        /// The title the user gave, if any.
        title: Option<String>,
    },
    /// Set the session's model.
    SetModel {
        /// The session to change.
        session: SessionKey,
        /// The provider the model lives in.
        provider: String,
        /// The model id.
        model: String,
    },
    /// Set the session's reasoning effort.
    SetReasoning {
        /// The session to change.
        session: SessionKey,
        /// The effort, `auto` / `low` / `medium` / `high`.
        effort: String,
    },
    /// Refetch the catalogue views for the current session.
    RefreshViews,
    /// Refetch only the plans and jobs shown in the rail.
    RefreshRail,
    /// Answer the question the turn is parked on.
    AnswerQuestion {
        /// The session the question belongs to.
        session: SessionKey,
        /// The host's id for the question.
        question: String,
        /// The answer text.
        answer: QuestionAnswer,
    },
    /// Attach to an existing session, leaving the current stream behind.
    OpenSession {
        /// The session to attach to.
        session: Box<norte_code_wire::SessionSummary>,
    },
    /// Run a plugin operation on the host; completion re-lists the plugins.
    Plugin(PluginOp),
    /// Rename a session in the host's list.
    RenameSession {
        /// The session to rename.
        session: norte_code_wire::SessionKey,
        /// The new title.
        title: String,
    },
    /// Delete a session in the host's list.
    DeleteSession {
        /// The session to delete.
        session: norte_code_wire::SessionKey,
    },
    /// Ask the host to cancel the running turn.
    CancelTurn {
        /// The session running the turn.
        session: norte_code_wire::SessionKey,
        /// The turn the host assigned.
        turn_id: String,
    },
    /// Compact the attached session now.
    Compact {
        /// The session to compact.
        session: norte_code_wire::SessionKey,
    },
    /// Fork the session, optionally at a turn.
    Fork {
        /// The session to fork.
        session: norte_code_wire::SessionKey,
        /// The turn to fork from, when known.
        turn: Option<u64>,
    },
    /// Rewind the session to a turn.
    Rewind {
        /// The session to rewind.
        session: norte_code_wire::SessionKey,
        /// The turn to rewind to, when known.
        turn: Option<u64>,
    },
    /// Store a provider key on the host.
    SaveSecret {
        /// The provider the key belongs to.
        provider: String,
        /// The secret value.
        key: String,
    },
}

/// A plugin operation the host can do for us.
#[derive(Clone, Debug)]
pub(crate) enum PluginOp {
    /// Install a plugin from a git repository, optionally one package of it.
    Install {
        /// The repository URL.
        url: String,
        /// One workspace package to install instead of the whole workspace.
        package: Option<String>,
    },
    /// Update one plugin, or every plugin with a recorded source.
    Update {
        /// The plugin to update; `None` updates all.
        name: Option<String>,
    },
    /// Remove one plugin.
    Remove {
        /// The plugin to remove.
        name: String,
    },
    /// Enable or disable one plugin.
    SetEnabled {
        /// The plugin to toggle.
        name: String,
        /// What it should be after the change.
        enabled: bool,
    },
}

/// Ask the host to cancel the running turn.
async fn cancel_turn(
    api: &Arc<RestClient>,
    events: &Sender<ClientEvent>,
    target: &SessionKey,
    turn_id: String,
) {
    let result = match norte_code_client::TurnId::new(&turn_id) {
        Ok(turn) => api
            .cancel_turn(target.as_str(), &turn)
            .await
            .map_err(ClientError::cancel),
        Err(error) => Err(ClientError::invalid_turn(error)),
    };
    if let Err(reason) = result {
        let _ = events.send(ClientEvent::SendFailed { reason }).await;
    }
}

enum BackgroundOp {
    /// A plugin operation.
    Plugin(PluginOp),
    /// A session rename.
    Rename {
        /// The session to rename.
        session: norte_code_wire::SessionKey,
        /// The new title.
        title: String,
    },
    /// A session delete.
    Delete {
        /// The session to delete.
        session: norte_code_wire::SessionKey,
    },
}

/// Spawn a host operation without blocking the command loop.
fn spawn_host_op(kind: BackgroundOp, api: Arc<RestClient>, events: Sender<ClientEvent>) {
    tokio::spawn(async move {
        match kind {
            BackgroundOp::Plugin(op) => run_plugin_op(&api, op, &events).await,
            BackgroundOp::Rename { session, title } => {
                rename_session(&api, &session, &title, &events).await;
            }
            BackgroundOp::Delete { session } => delete_session(&api, &session, &events).await,
        }
    });
}

/// Rename a session; on any outcome re-list the sessions.
async fn rename_session(
    api: &Arc<RestClient>,
    session: &norte_code_wire::SessionKey,
    title: &str,
    events: &Sender<ClientEvent>,
) {
    match api.rename_session(session.as_str(), title).await {
        Ok(()) => {
            let _ = events
                .send(ClientEvent::Notice {
                    text: format!("session renamed · {title}"),
                })
                .await;
        }
        Err(error) => {
            let _ = events
                .send(ClientEvent::SendFailed {
                    reason: ClientError::rename(error),
                })
                .await;
        }
    }
    if let Ok(sessions) = api.sessions().await {
        let _ = events.send(ClientEvent::Sessions(sessions)).await;
    }
}

/// Delete a session; on any outcome re-list the sessions.
async fn delete_session(
    api: &Arc<RestClient>,
    session: &norte_code_wire::SessionKey,
    events: &Sender<ClientEvent>,
) {
    match api.delete_session(session.as_str()).await {
        Ok(()) => {
            let _ = events
                .send(ClientEvent::Notice {
                    text: "session deleted".to_string(),
                })
                .await;
        }
        Err(error) => {
            let _ = events
                .send(ClientEvent::SendFailed {
                    reason: ClientError::delete(error),
                })
                .await;
        }
    }
    if let Ok(sessions) = api.sessions().await {
        let _ = events.send(ClientEvent::Sessions(sessions)).await;
    }
}

/// Run one plugin operation and re-list the plugins afterwards.
async fn run_plugin_op(api: &Arc<RestClient>, op: PluginOp, events: &Sender<ClientEvent>) {
    let outcome = match &op {
        PluginOp::Install { url, package } => {
            api.install_plugin(url, package.as_deref(), false).await
        }
        PluginOp::Update { name } => api.update_plugin(name.as_deref()).await,
        PluginOp::Remove { name } => api.remove_plugin(name).await,
        PluginOp::SetEnabled { name, enabled } => api.set_plugin_enabled(name, *enabled).await,
    };
    match outcome {
        Ok(change) => {
            let _ = events
                .send(ClientEvent::Notice {
                    text: format!("plugin {} · {}", change.name, change.operation),
                })
                .await;
        }
        Err(error) => {
            let _ = events
                .send(ClientEvent::SendFailed {
                    reason: ClientError::plugin(error),
                })
                .await;
        }
    }
    if let Ok(plugins) = api.plugins().await {
        let _ = events.send(ClientEvent::Plugins(plugins)).await;
    }
}

/// Fetch everything the interface needs right after attach.
async fn fetch_views(api: &Arc<RestClient>, session: &str, events: &Sender<ClientEvent>) {
    let (
        models,
        plans,
        jobs,
        telemetry_global,
        plugins,
        catalog,
        agents,
        cost_session,
        cost_global,
        server_info,
        liveness,
        readiness,
        quota,
    ) = tokio::join!(
        api.models(),
        api.plans(session),
        api.list_jobs(session),
        api.telemetry(None),
        api.plugins(),
        api.catalog_detail(),
        api.agents(),
        api.cost_report(Some(session)),
        api.cost_report(None),
        api.info(),
        api.health_live(),
        api.health_ready(),
        api.quota(),
    );
    if let Ok(models) = models {
        let _ = events.send(ClientEvent::Models(models)).await;
    }
    if let Ok(plans) = plans {
        let _ = events.send(ClientEvent::Plans(plans)).await;
    }
    if let Ok(jobs) = jobs {
        let _ = events.send(ClientEvent::Jobs(jobs)).await;
    }
    if let Ok(report) = telemetry_global {
        let _ = events.send(ClientEvent::Telemetry(Arc::new(report))).await;
    }
    if let Ok(plugins) = plugins {
        let _ = events.send(ClientEvent::Plugins(plugins)).await;
    }
    if let Ok(catalog) = catalog {
        let _ = events.send(ClientEvent::Catalog(catalog)).await;
    }
    if let Ok(agents) = agents {
        let _ = events.send(ClientEvent::Agents(agents)).await;
    }
    if let Ok(cost) = cost_session {
        let _ = events.send(ClientEvent::CostSession(cost)).await;
    }
    if let Ok(cost) = cost_global {
        let _ = events.send(ClientEvent::CostGlobal(cost)).await;
    }
    if let Ok(info) = server_info {
        let _ = events.send(ClientEvent::ServerInfo(info)).await;
    }
    if let Ok(live) = liveness {
        let _ = events.send(ClientEvent::Liveness(live)).await;
    }
    if let Ok(ready) = readiness {
        let _ = events.send(ClientEvent::Readiness(ready)).await;
    }
    if let Ok(quota) = quota {
        let _ = events.send(ClientEvent::Quota(quota)).await;
    }
}

async fn create_session(
    client: &Arc<RemoteClient>,
    events: &Sender<ClientEvent>,
    title: Option<String>,
) -> Option<StreamOutcome> {
    match client.create_session(title.as_deref()).await {
        Ok(session) => Some(StreamOutcome::Switch(Box::new(session))),
        Err(error) => {
            let _ = events
                .send(ClientEvent::SendFailed {
                    reason: ClientError::create_session(error),
                })
                .await;
            None
        }
    }
}

/// Refetch the session's plans and jobs after the agent changes them.
async fn refresh_rail(api: &Arc<RestClient>, session: &str, events: &Sender<ClientEvent>) {
    let (plans, jobs) = tokio::join!(api.plans(session), api.list_jobs(session));
    if let Ok(plans) = plans {
        let _ = events.send(ClientEvent::Plans(plans)).await;
    }
    if let Ok(jobs) = jobs {
        let _ = events.send(ClientEvent::Jobs(jobs)).await;
    }
}

/// Starts the connection task and returns (commands, events).
pub(crate) fn start(config: ClientConfig) -> (Sender<UiCommand>, Receiver<ClientEvent>) {
    let (events_tx, events_rx) = mpsc::channel(256);
    let (commands_tx, commands_rx) = mpsc::channel(64);
    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Runtime::new() {
            Ok(runtime) => runtime,
            Err(error) => {
                let _ = events_tx.blocking_send(ClientEvent::Detached {
                    reason: ClientError::runtime(error),
                });
                return;
            }
        };
        runtime.block_on(connection(config, commands_rx, events_tx));
    });
    (commands_tx, events_rx)
}

/// Owns the connection; driven by commands from the interface.
async fn connection(
    config: ClientConfig,
    mut commands: Receiver<UiCommand>,
    events: Sender<ClientEvent>,
) {
    let reconnect = config.reconnect;
    let client = match RemoteClient::new(
        config.service.url.clone(),
        config.service.token.as_deref(),
        None,
    ) {
        Ok(client) => Arc::new(client.with_agent_id(config.agent_id.clone())),
        Err(error) => {
            let _ = events
                .send(ClientEvent::Detached {
                    reason: ClientError::remote_client(error),
                })
                .await;
            return;
        }
    };
    let api = match RestClient::new(
        &config.service.url,
        config.service.token.as_deref().unwrap_or_default(),
    ) {
        Ok(api) => Arc::new(api),
        Err(error) => {
            let _ = events
                .send(ClientEvent::Detached {
                    reason: ClientError::rest_client(error),
                })
                .await;
            return;
        }
    };
    let sessions = match client.list_sessions().await {
        Ok(sessions) => sessions,
        Err(error) => {
            let _ = events
                .send(ClientEvent::Detached {
                    reason: ClientError::list_sessions(error),
                })
                .await;
            return;
        }
    };
    let fresh = match client.create_session(None).await {
        Ok(fresh) => fresh,
        Err(error) => {
            let _ = events
                .send(ClientEvent::Detached {
                    reason: ClientError::create_session(error),
                })
                .await;
            return;
        }
    };
    let _ = events.send(ClientEvent::Sessions(sessions)).await;
    let mut session = attach_shared(&client, &events, Box::new(fresh)).await;
    loop {
        match stream_loop(&client, &api, &mut commands, &events, &session, &reconnect).await {
            StreamOutcome::Retry => return,
            StreamOutcome::Switch(summary) => {
                session = attach_shared(&client, &events, summary).await;
            }
        }
    }
}

/// Largest attachment the client will read before asking the host to store it.
const MAX_ATTACHMENT_BYTES: u64 = 16 * 1024 * 1024;

/// Send a user message on the session's stream.
async fn send_prompt(
    api: &Arc<RestClient>,
    events: &Sender<ClientEvent>,
    target: &SessionKey,
    text: String,
    parts: Vec<UserInputPart>,
    queue_mode: String,
) {
    let content = (!parts.is_empty()).then_some(parts);
    if let Err(error) = api
        .send_prompt(target.as_str(), Some(&text), content, None, &queue_mode)
        .await
    {
        let _ = events
            .send(ClientEvent::SendFailed {
                reason: ClientError::send_prompt(error),
            })
            .await;
    }
}

/// Store one attachment and report the receipt the chip will carry.
async fn upload_attachment(
    api: &Arc<RestClient>,
    events: &Sender<ClientEvent>,
    target: &SessionKey,
    attachment: PreparedAttachment,
) {
    let bytes = match &attachment.source {
        AttachmentSource::Image(bytes) => bytes.clone(),
        AttachmentSource::File(path) => match tokio::fs::read(path).await {
            Ok(bytes) => bytes,
            Err(error) => {
                let _ = events
                    .send(ClientEvent::UploadFailed {
                        reason: ClientError::read_attachment(path, error),
                    })
                    .await;
                return;
            }
        },
    };
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_ATTACHMENT_BYTES {
        let _ = events
            .send(ClientEvent::UploadFailed {
                reason: ClientError::attachment_too_large(bytes.len(), MAX_ATTACHMENT_BYTES),
            })
            .await;
        return;
    }
    match api
        .upload(target.as_str(), attachment.mime.as_str(), bytes)
        .await
    {
        Ok(receipt) => {
            let _ = events
                .send(ClientEvent::Uploaded {
                    attachment,
                    receipt,
                })
                .await;
        }
        Err(error) => {
            let _ = events
                .send(ClientEvent::UploadFailed {
                    reason: ClientError::upload(error),
                })
                .await;
        }
    }
}

/// Answer the question the turn is parked on.
async fn answer_question(
    api: &Arc<RestClient>,
    events: &Sender<ClientEvent>,
    target: &SessionKey,
    question: String,
    answer: QuestionAnswer,
) {
    if let Err(error) = api
        .answer_question(target.as_str(), &question, answer.as_str())
        .await
    {
        let _ = events
            .send(ClientEvent::SendFailed {
                reason: ClientError::answer(error),
            })
            .await;
    }
}

/// Ask the host to compact a session now; failures reach the transcript.
async fn compact_session(
    api: &RestClient,
    session: &norte_code_wire::SessionKey,
    events: &Sender<ClientEvent>,
) {
    if let Err(error) = api.compact(session.as_str()).await {
        let _ = events
            .send(ClientEvent::SendFailed {
                reason: ClientError::compact(error),
            })
            .await;
    }
}

async fn fork_session(
    client: &RemoteClient,
    api: &RestClient,
    session: &norte_code_wire::SessionKey,
    turn: Option<u64>,
    events: &Sender<ClientEvent>,
) {
    let id = turn.map(|turn| format!("turn_{turn}"));
    match api.fork(session.as_str(), id.as_deref(), None).await {
        Ok(forked) => {
            // The fork is a real session in the host's list; find its summary
            // so the app can attach to it instead of leaving us on the original.
            let summary = client
                .list_sessions()
                .await
                .ok()
                .and_then(|sessions| {
                    sessions
                        .into_iter()
                        .find(|candidate| candidate.session_id == forked)
                })
                .map(Box::new);
            if let Some(session) = summary {
                let _ = events.send(ClientEvent::Forked { session }).await;
            } else {
                let _ = events
                    .send(ClientEvent::Notice {
                        text: format!("forked session {forked} · reopen it from Sessions"),
                    })
                    .await;
            }
        }
        Err(error) => {
            let _ = events
                .send(ClientEvent::SendFailed {
                    reason: ClientError::fork(error),
                })
                .await;
        }
    }
}

async fn rewind_session(
    api: &RestClient,
    session: &norte_code_wire::SessionKey,
    turn: Option<u64>,
    events: &Sender<ClientEvent>,
) {
    let Some(turn) = turn else {
        let _ = events
            .send(ClientEvent::Notice {
                text: "rewind needs a turn from a replayed message".to_owned(),
            })
            .await;
        return;
    };
    match api
        .rewind(session.as_str(), &format!("turn_{turn}"), true)
        .await
    {
        Ok(rewound) => {
            let _ = events
                .send(ClientEvent::Notice {
                    text: format!("rewound {} turns", rewound.len()),
                })
                .await;
        }
        Err(error) => {
            let _ = events
                .send(ClientEvent::SendFailed {
                    reason: ClientError::rewind(error),
                })
                .await;
        }
    }
}

/// Send the attachment frames for a session.
async fn attach_shared(
    client: &Arc<RemoteClient>,
    events: &Sender<ClientEvent>,
    summary: Box<norte_code_wire::SessionSummary>,
) -> SessionKey {
    let session = SessionKey::new(summary.session_id.clone());
    let _ = events
        .send(ClientEvent::Attached { session: *summary })
        .await;
    if let Ok(sessions) = client.list_sessions().await {
        let _ = events.send(ClientEvent::Sessions(sessions)).await;
    }
    session
}

/// Fetch the session's history; best effort, failure is reported.
async fn fetch_history(api: &Arc<RestClient>, session: &str, events: &Sender<ClientEvent>) {
    match api.history(session).await {
        Ok(items) => {
            let _ = events.send(ClientEvent::History(items)).await;
        }
        Err(error) => {
            let _ = events
                .send(ClientEvent::HistoryFailed {
                    reason: ClientError::history(error),
                })
                .await;
        }
    }
}

fn retry_delay(reconnect: &ReconnectConfig, attempt: u32) -> Duration {
    Duration::from_millis((reconnect.retry_base_ms << attempt.min(6)).min(reconnect.retry_max_ms))
}

enum StreamOutcome {
    /// The stream closed; the caller should re-open it.
    Retry,
    /// The user asked another session.
    Switch(Box<norte_code_wire::SessionSummary>),
}

/// Owns the session's event stream; driven by both the stream and commands.
///
/// Reconnects forever with a capped backoff: the server supports resume
/// from `last-event-id`, so a close is only a hiccup. The interface hears
/// about sustained trouble once, not per attempt; Ctrl+R forces a fresh
/// attach from the session list.
async fn stream_loop(
    client: &Arc<RemoteClient>,
    api: &Arc<RestClient>,
    commands: &mut Receiver<UiCommand>,
    events: &Sender<ClientEvent>,
    session: &SessionKey,
    reconnect: &ReconnectConfig,
) -> StreamOutcome {
    let healthy_stream = Duration::from_secs(reconnect.healthy_stream_s);
    let mut attempt = 0u32;
    let mut announced = false;
    let mut primed = false;
    loop {
        match client.open_events(session.as_str()).await {
            Ok(mut stream) => {
                if !primed {
                    // The host only honours these once the stream is attached.
                    Box::pin(fetch_history(api, session.as_str(), events)).await;
                    Box::pin(fetch_views(api, session.as_str(), events)).await;
                    primed = true;
                }
                let opened_at = std::time::Instant::now();
                match Box::pin(drive_stream(
                    &mut stream,
                    commands,
                    events,
                    client,
                    api,
                    session,
                ))
                .await
                {
                    StreamOutcome::Switch(summary) => return StreamOutcome::Switch(summary),
                    StreamOutcome::Retry => {}
                }
                if opened_at.elapsed() >= healthy_stream {
                    attempt = 0;
                    announced = false;
                } else {
                    attempt = attempt.saturating_add(1);
                }
                if attempt >= reconnect.give_up_attempts && !announced {
                    let _ = events
                        .send(ClientEvent::Detached {
                            reason: ClientError::unstable_stream(),
                        })
                        .await;
                    announced = true;
                }
            }
            Err(error) => {
                attempt = attempt.saturating_add(1);
                if attempt >= reconnect.give_up_attempts && !announced {
                    let _ = events
                        .send(ClientEvent::Detached {
                            reason: ClientError::lost_stream(error),
                        })
                        .await;
                    announced = true;
                }
            }
        }
        tokio::time::sleep(retry_delay(reconnect, attempt)).await;
    }
}

async fn set_model_and_refresh(
    client: &RemoteClient,
    api: &RestClient,
    events: &Sender<ClientEvent>,
    session: &SessionKey,
    provider: &str,
    model: &str,
) {
    let _ = api
        .set_session_model(session.as_str(), provider, model)
        .await;
    if let Ok(models) = api.models().await {
        let _ = events.send(ClientEvent::Models(models)).await;
    }
    if let Ok(sessions) = client.list_sessions().await {
        let _ = events.send(ClientEvent::Sessions(sessions)).await;
    }
}

async fn save_secret(api: &RestClient, events: &Sender<ClientEvent>, provider: &str, key: &str) {
    match api.save_secret(provider, key).await {
        Ok(()) => {
            let _ = events
                .send(ClientEvent::SecretSaved {
                    provider: provider.to_owned(),
                })
                .await;
        }
        Err(error) => {
            let _ = events
                .send(ClientEvent::SecretSaveFailed {
                    reason: ClientError::save_secret(error),
                })
                .await;
        }
    }
}

/// Pump events from the stream while dispatching commands.
async fn apply_command(
    command: UiCommand,
    active: &SessionKey,
    client: &Arc<RemoteClient>,
    api: &Arc<RestClient>,
    events: &Sender<ClientEvent>,
) -> Option<StreamOutcome> {
    match command {
        UiCommand::OpenSession { session } => return Some(StreamOutcome::Switch(session)),
        UiCommand::Plugin(op) => {
            spawn_host_op(BackgroundOp::Plugin(op), Arc::clone(api), events.clone());
        }
        UiCommand::RenameSession { session, title } => {
            spawn_host_op(
                BackgroundOp::Rename { session, title },
                Arc::clone(api),
                events.clone(),
            );
        }
        UiCommand::DeleteSession { session } => {
            spawn_host_op(
                BackgroundOp::Delete { session },
                Arc::clone(api),
                events.clone(),
            );
        }
        UiCommand::CancelTurn { session, turn_id } => {
            cancel_turn(api, events, &session, turn_id).await;
        }
        UiCommand::Compact { session } => {
            compact_session(api, &session, events).await;
        }
        UiCommand::Fork { session, turn } => {
            fork_session(client, api, &session, turn, events).await;
        }
        UiCommand::Rewind { session, turn } => {
            rewind_session(api, &session, turn, events).await;
        }
        UiCommand::Send {
            session,
            text,
            parts,
            queue_mode,
        } => {
            send_prompt(api, events, &session, text, parts, queue_mode).await;
        }
        UiCommand::Upload {
            session,
            attachment,
        } => {
            upload_attachment(api, events, &session, attachment).await;
        }
        UiCommand::Refresh => {
            if let Ok(sessions) = client.list_sessions().await {
                let _ = events.send(ClientEvent::Sessions(sessions)).await;
            }
        }
        UiCommand::CreateSession { title } => {
            return create_session(client, events, title).await;
        }
        UiCommand::AnswerQuestion {
            session,
            question,
            answer,
        } => {
            answer_question(api, events, &session, question, answer).await;
        }
        UiCommand::RefreshViews => {
            Box::pin(fetch_views(api, active.as_str(), events)).await;
        }
        UiCommand::RefreshRail => {
            refresh_rail(api, active.as_str(), events).await;
        }
        UiCommand::SetModel {
            session,
            provider,
            model,
        } => {
            set_model_and_refresh(client, api, events, &session, &provider, &model).await;
        }
        UiCommand::SaveSecret { provider, key } => {
            save_secret(api, events, &provider, &key).await;
        }
        UiCommand::SetReasoning { session, effort } => {
            let reasoning = match effort.as_str() {
                "low" | "medium" | "high" => norte_code_wire::Reasoning::Effort { effort },
                _ => norte_code_wire::Reasoning::Automatic,
            };
            let _ = api
                .set_reasoning(
                    session.as_str(),
                    norte_code_wire::ReasoningScope::Session,
                    reasoning,
                )
                .await;
        }
    }
    None
}

async fn drive_stream(
    stream: &mut EventStream,
    commands: &mut Receiver<UiCommand>,
    events: &Sender<ClientEvent>,
    client: &Arc<RemoteClient>,
    api: &Arc<RestClient>,
    session: &SessionKey,
) -> StreamOutcome {
    loop {
        tokio::select! {
            Some(command) = commands.recv() => {
                if let Some(outcome) = apply_command(command, session, client, api, events).await {
                    return outcome;
                }
            }
            frame = stream.next() => {
                match frame {
                    Some(event) => {
                        let _ = events.send(ClientEvent::Server(Box::new(event))).await;
                    }
                    None => return StreamOutcome::Retry,
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
