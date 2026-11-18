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
use norte_code_wire::{SessionKey, SessionSummary};
use tokio::sync::mpsc::{self, Receiver, Sender};

use crate::config::ClientConfig;

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
    Telemetry(norte_code_wire::TelemetryReport),
    /// Telemetry for the attached session.
    TelemetrySession(norte_code_wire::TelemetryReport),
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
        /// `steer` or `queue`; forwarded to the host.
        queue_mode: String,
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
}

const MAX_ATTACH_ATTEMPTS: u32 = 3;
const RETRY_BASE_MS: u64 = 400;

/// Fetch everything the interface needs right after attach.
async fn fetch_views(api: &Arc<RestClient>, session: &str, events: &Sender<ClientEvent>) {
    let (
        models,
        plans,
        jobs,
        telemetry_session,
        telemetry_global,
        plugins,
        catalog,
        agents,
        cost_session,
        cost_global,
    ) = tokio::join!(
        api.models(),
        api.plans(session),
        api.list_jobs(session),
        api.telemetry(Some(session)),
        api.telemetry(None),
        api.plugins(),
        api.catalog_detail(),
        api.agents(),
        api.cost_report(Some(session)),
        api.cost_report(None),
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
    if let Ok(report) = telemetry_session {
        let _ = events.send(ClientEvent::TelemetrySession(report)).await;
    }
    if let Ok(report) = telemetry_global {
        let _ = events.send(ClientEvent::Telemetry(report)).await;
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
    let session = SessionKey::new(fresh.session_id.clone());
    let _ = events.send(ClientEvent::Sessions(sessions)).await;
    let _ = events.send(ClientEvent::Attached { session: fresh }).await;
    // The views are filled from the same REST calls the service serves.
    Box::pin(fetch_views(&api, session.as_str(), &events)).await;
    Box::pin(stream_loop(&client, &api, &mut commands, &events, &session)).await;
}

/// Owns the session's event stream; driven by both the stream and commands.
async fn stream_loop(
    client: &Arc<RemoteClient>,
    api: &Arc<RestClient>,
    commands: &mut Receiver<UiCommand>,
    events: &Sender<ClientEvent>,
    session: &SessionKey,
) {
    let mut attempt = 0u32;
    loop {
        match client.open_events(session.as_str()).await {
            Ok(mut stream) => {
                let finished = Box::pin(drive_stream(
                    &mut stream,
                    commands,
                    events,
                    client,
                    api,
                    session,
                ))
                .await;
                if finished {
                    return;
                }
                attempt = attempt.saturating_add(1);
            }
            Err(error) => {
                attempt = attempt.saturating_add(1);
                if attempt > MAX_ATTACH_ATTEMPTS {
                    let _ = events
                        .send(ClientEvent::Detached {
                            reason: format!("could not subscribe to the stream: {error}"),
                        })
                        .await;
                    return;
                }
            }
        }
        if attempt > MAX_ATTACH_ATTEMPTS {
            let _ = events
                .send(ClientEvent::Detached {
                    reason: "the stream keeps closing; check the raep server".into(),
                })
                .await;
            return;
        }
        tokio::time::sleep(Duration::from_millis(RETRY_BASE_MS * (1 << attempt.min(4)))).await;
    }
}

/// Pump events from the stream while dispatching commands; returns `true`
/// when the caller should stop (the turn completed and the stream closed).
async fn drive_stream(
    stream: &mut EventStream,
    commands: &mut Receiver<UiCommand>,
    events: &Sender<ClientEvent>,
    client: &Arc<RemoteClient>,
    api: &Arc<RestClient>,
    session: &SessionKey,
) -> bool {
    loop {
        tokio::select! {
            Some(command) = commands.recv() => {
                match command {
                    UiCommand::Send { session: target, text, queue_mode } => {
                        let _ = api
                            .send_prompt(
                                target.as_str(),
                                Some(&text),
                                None::<Vec<UserInputPart>>,
                                None,
                                &queue_mode,
                            )
                            .await;
                    }
                    UiCommand::Refresh => {
                        if let Ok(sessions) = client.list_sessions().await {
                            let _ = events.send(ClientEvent::Sessions(sessions)).await;
                        }
                    }
                    UiCommand::RefreshViews => {
                        Box::pin(fetch_views(api, session.as_str(), events)).await;
                    }
                    UiCommand::SetModel {
                        session: target,
                        provider,
                        model,
                    } => {
                        let _ = api.set_session_model(target.as_str(), &provider, &model).await;
                        if let Ok(models) = api.models().await {
                            let _ = events.send(ClientEvent::Models(models)).await;
                        }
                    }
                    UiCommand::SetReasoning {
                        session: target,
                        effort,
                    } => {
                        let reasoning = match effort.as_str() {
                            "low" | "medium" | "high" => norte_code_wire::Reasoning::Effort {
                                effort: effort.clone(),
                            },
                            _ => norte_code_wire::Reasoning::Automatic,
                        };
                        let _ = api
                            .set_reasoning(
                                target.as_str(),
                                norte_code_wire::ReasoningScope::Session,
                                reasoning,
                            )
                            .await;
                    }
                }
            }
            frame = stream.next() => {
                match frame {
                    Some(event) => {
                        let _ = events.send(ClientEvent::Server(Box::new(event))).await;
                    }
                    None => return false,
                }
            }
        }
    }
}
