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
use norte_code_client::{AgentClient, EventStream, RemoteClient, RestClient};
use norte_code_wire::UserInputPart;
use norte_code_wire::{SessionKey, SessionSummary, UploadReceipt};
use tokio::sync::mpsc::{self, Receiver, Sender};

use crate::attachment::{AttachmentSource, PreparedAttachment};
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
        reason: String,
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
        reason: String,
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
        reason: String,
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
        reason: String,
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
    /// Answer the question the turn is parked on.
    AnswerQuestion {
        /// The session the question belongs to.
        session: SessionKey,
        /// The host's id for the question.
        question: String,
        /// The answer text.
        answer: String,
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

const RETRY_BASE_MS: u64 = 400;

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
            .map_err(|error| error.to_string()),
        Err(error) => Err(format!("the running turn has an unusable id: {error}")),
    };
    if let Err(reason) = result {
        let _ = events
            .send(ClientEvent::SendFailed {
                reason: format!("cancel refused: {reason}"),
            })
            .await;
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
                    reason: format!("rename session failed · {error}"),
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
                    reason: format!("delete session failed · {error}"),
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
                    reason: format!("plugin operation failed · {error}"),
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

/// Starts the connection task and returns (commands, events).
pub(crate) fn start(config: ClientConfig) -> (Sender<UiCommand>, Receiver<ClientEvent>) {
    let (events_tx, events_rx) = mpsc::channel(256);
    let (commands_tx, commands_rx) = mpsc::channel(64);
    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Runtime::new() {
            Ok(runtime) => runtime,
            Err(error) => {
                let _ = events_tx.blocking_send(ClientEvent::Detached {
                    reason: format!("could not start the runtime: {error}"),
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
    let client = match RemoteClient::new(
        config.service.url.clone(),
        config.service.token.as_deref(),
        None,
    ) {
        Ok(client) => Arc::new(client.with_agent_id(config.agent_id.clone())),
        Err(error) => {
            let _ = events
                .send(ClientEvent::Detached {
                    reason: format!("could not build the raep client: {error}"),
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
                    reason: format!("could not build the raep api: {error}"),
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
                    reason: format!("could not list the sessions: {error}"),
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
                    reason: format!("could not create a session: {error}"),
                })
                .await;
            return;
        }
    };
    let _ = events.send(ClientEvent::Sessions(sessions)).await;
    let mut session = attach_shared(&client, &events, Box::new(fresh)).await;
    loop {
        match stream_loop(&client, &api, &mut commands, &events, &session).await {
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
                reason: format!("message refused: {error}"),
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
                        reason: format!("could not read {}: {error}", path.display()),
                    })
                    .await;
                return;
            }
        },
    };
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_ATTACHMENT_BYTES {
        let _ = events
            .send(ClientEvent::UploadFailed {
                reason: format!(
                    "attachment has {} bytes, limit is {MAX_ATTACHMENT_BYTES}",
                    bytes.len()
                ),
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
                    reason: error.to_string(),
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
    answer: String,
) {
    if let Err(error) = api
        .answer_question(target.as_str(), &question, &answer)
        .await
    {
        let _ = events
            .send(ClientEvent::SendFailed {
                reason: format!("answer refused: {error}"),
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
                reason: format!("compact refused: {error}"),
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
                    reason: format!("fork failed: {error}"),
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
                    reason: format!("rewind failed: {error}"),
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
                    reason: error.to_string(),
                })
                .await;
        }
    }
}

/// After this many consecutive failures or short-lived streams, surface a
/// notice once; the reconnect keeps going in the background regardless.
const GIVE_UP_ATTEMPTS: u32 = 3;
/// A stream that stays up at least this long counts as healthy.
const HEALTHY_STREAM: Duration = Duration::from_secs(10);
const RETRY_MAX_MS: u64 = 5000;

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
) -> StreamOutcome {
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
                if opened_at.elapsed() >= HEALTHY_STREAM {
                    attempt = 0;
                    announced = false;
                } else {
                    attempt = attempt.saturating_add(1);
                }
                if attempt >= GIVE_UP_ATTEMPTS && !announced {
                    let _ = events
                        .send(ClientEvent::Detached {
                            reason: "the stream keeps closing; retrying in the background — check the raep server".into(),
                        })
                        .await;
                    announced = true;
                }
            }
            Err(error) => {
                attempt = attempt.saturating_add(1);
                if attempt >= GIVE_UP_ATTEMPTS && !announced {
                    let _ = events
                        .send(ClientEvent::Detached {
                            reason: format!(
                                "lost the stream ({error}); retrying in the background — check the raep server"
                            ),
                        })
                        .await;
                    announced = true;
                }
            }
        }
        let delay = (RETRY_BASE_MS << attempt.min(6)).min(RETRY_MAX_MS);
        tokio::time::sleep(Duration::from_millis(delay)).await;
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
        UiCommand::SetModel {
            session,
            provider,
            model,
        } => {
            let _ = api
                .set_session_model(session.as_str(), &provider, &model)
                .await;
            if let Ok(models) = api.models().await {
                let _ = events.send(ClientEvent::Models(models)).await;
            }
            if let Ok(sessions) = client.list_sessions().await {
                let _ = events.send(ClientEvent::Sessions(sessions)).await;
            }
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
