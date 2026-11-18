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
use norte_code_wire::{SessionKey, SessionSummary};
use norte_code_wire::UserInputPart;
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
}

/// Commands the interface sends to the connection task.
#[derive(Clone, Debug)]
pub(crate) enum UiCommand {
    /// Attach to a fresh session; used on startup and after a detach.
    Attach,
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
}

const MAX_ATTACH_ATTEMPTS: u32 = 3;
const RETRY_BASE_MS: u64 = 400;

/// Starts the connection task and returns (commands, events).
pub(crate) fn start(
    config: ClientConfig,
) -> (Sender<UiCommand>, Receiver<ClientEvent>) {
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
    let api = match RestClient::new(&config.service.url, config.service.token.as_deref().unwrap_or_default()) {
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
    let _ = events
        .send(ClientEvent::Attached {
            session: fresh,
        })
        .await;
    stream_loop(&client, &api, &mut commands, &events, &session).await;
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
                let finished = drive_stream(&mut stream, commands, events, client, api).await;
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
                    UiCommand::Attach => {}
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
