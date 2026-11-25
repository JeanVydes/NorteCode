//! HTTP handlers for the RAEP websocket.

#[allow(clippy::wildcard_imports)]
use super::*;

pub(super) async fn websocket(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    let supports_raep_v1 = headers
        .get("sec-websocket-protocol")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|protocol| protocol.trim() == "raep.v1")
        });
    if !supports_raep_v1 {
        return StatusCode::UPGRADE_REQUIRED.into_response();
    }
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    let Some(permit) = host.try_connection() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    upgrade
        .protocols(["raep.v1"])
        .max_message_size(host.max_frame_bytes())
        .on_upgrade(move |socket| websocket_session(host, socket, permit, principal))
        .into_response()
}

#[allow(clippy::too_many_lines)]
async fn websocket_session(
    host: Arc<NorteCodeRaepHost>,
    mut socket: WebSocket,
    _permit: tokio::sync::OwnedSemaphorePermit,
    principal: String,
) {
    if send_frame(&mut socket, host.hello()).await.is_err() {
        return;
    }
    let mut attached_sessions = std::collections::HashMap::new();
    let mut heartbeat = tokio::time::interval_at(
        tokio::time::Instant::now() + std::time::Duration::from_secs(20),
        std::time::Duration::from_secs(20),
    );
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_pong = tokio::time::Instant::now();
    'connection: loop {
        tokio::select! {
            message = socket.recv() => {
                let Some(Ok(message)) = message else { break };
                match message {
            Message::Text(text) => {
                if text.len() > host.max_frame_bytes() {
                    close_socket(&mut socket, 1009, "RAEP frame exceeds size limit").await;
                    break 'connection;
                }
                let Ok(frame) = serde_json::from_str::<Envelope<Value>>(&text) else {
                    close_socket(&mut socket, 4400, "malformed RAEP JSON frame").await;
                    break 'connection;
                };
                if frame.version != raep_types::WIRE_VERSION {
                    close_socket(&mut socket, 4426, "unsupported RAEP wire version").await;
                    break 'connection;
                }
                let mut session_hint = frame.session_id.clone();
                let request_id = frame.id.clone();
                let is_attach = frame.kind == FrameType::Standard(StandardFrameType::SessionAttach);
                let is_detach = matches!(
                    &frame.kind,
                    FrameType::Standard(
                        StandardFrameType::SessionDetach | StandardFrameType::SessionClose
                    )
                );
                let mut live = host.subscribe();
                let future = tokio::time::timeout(
                    host.command_timeout(),
                    host.handle_as(frame, Some(&principal)),
                );
                tokio::pin!(future);
                let mut emitted = std::collections::HashSet::new();
                loop {
                    tokio::select! {
                        result = &mut future => {
                            match result {
                                Ok(Ok(frames)) => {
                                    for frame in frames {
                                        if is_attach
                                            && frame.kind
                                                == FrameType::Standard(StandardFrameType::SessionAttached)
                                        {
                                            attached_sessions.insert(frame.session_id.clone(), request_id.clone());
                                            session_hint.clone_from(&frame.session_id);
                                        }
                                        if emitted.insert(frame.id.clone())
                                            && send_frame(&mut socket, frame).await.is_err()
                                        {
                                            break 'connection;
                                        }
                                    }
                                    if is_detach {
                                        attached_sessions.remove(&session_hint);
                                    }
                                }
                                Ok(Err(error)) => {
                                    if send_frame(
                                        &mut socket,
                                        correlated_error_frame(
                                            &error,
                                            Some(request_id),
                                            session_hint.clone(),
                                        ),
                                    )
                                    .await
                                    .is_err()
                                    {
                                        break 'connection;
                                    }
                                }
                                Err(_) => {
                                    let error = HostError::Service("command timed out".to_owned());
                                    if send_frame(
                                        &mut socket,
                                        correlated_error_frame(
                                            &error,
                                            Some(request_id),
                                            session_hint.clone(),
                                        ),
                                    )
                                    .await
                                    .is_err()
                                    {
                                        break 'connection;
                                    }
                                }
                            }
                            break;
                        }
                        result = live.recv() => {
                            match result {
                                Ok(frame)
                                    if frame.session_id == session_hint
                                        && emitted.insert(frame.id.clone()) =>
                                {
                                    if send_frame(&mut socket, frame).await.is_err() {
                                        break 'connection;
                                    }
                                }
                                Ok(_) => {}
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                    close_socket(&mut socket, 4008, "slow consumer; reconnect and resume").await;
                                    break 'connection;
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                    break 'connection;
                                }
                            }
                        }
                    }
                }
            }
            Message::Ping(payload) => {
                if socket.send(Message::Pong(payload)).await.is_err() {
                    break 'connection;
                }
            }
            Message::Pong(_) => last_pong = tokio::time::Instant::now(),
            Message::Binary(_) => {
                close_socket(&mut socket, 4400, "RAEP requires text JSON frames").await;
                break 'connection;
            }
            Message::Close(_) => break 'connection,
                }
            }
            _ = heartbeat.tick() => {
                if last_pong.elapsed() >= std::time::Duration::from_secs(60) {
                    close_socket(&mut socket, 1001, "websocket heartbeat timed out").await;
                    break 'connection;
                }
                if socket.send(Message::Ping(axum::body::Bytes::new())).await.is_err() {
                    break 'connection;
                }
            }
        }
    }
    for (session_id, attachment) in attached_sessions {
        host.detach_stream(&session_id, &attachment).await;
    }
}

async fn close_socket(socket: &mut WebSocket, code: u16, reason: &'static str) {
    let _ = socket
        .send(Message::Close(Some(axum::extract::ws::CloseFrame {
            code,
            reason: reason.into(),
        })))
        .await;
}

async fn send_frame(socket: &mut WebSocket, frame: Envelope<Value>) -> Result<(), ()> {
    let text = serde_json::to_string(&frame).map_err(|_| ())?;
    socket
        .send(Message::Text(text.into()))
        .await
        .map_err(|_| ())
}

pub(super) async fn commands(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(mut frame): Json<Envelope<Value>>,
) -> Response {
    tracing::info!(session = %session_id, "raep command received");
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    if frame.session_id.is_empty() {
        frame.session_id = session_id;
    } else if frame.session_id != session_id {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(payload(RejectedPayload {
                status: "error",
                accepted: Some(false),
                error: ErrorBody {
                    code: "INVALID_FRAME",
                    message: "session_id does not match path".to_owned(),
                },
            })),
        )
            .into_response();
    }
    let request_id = frame.id.clone();
    let request_session_id = frame.session_id.clone();
    // A message starts a turn, and a turn can outlive the request that started
    // it: a turn parked on `ask_user` does not end until a person answers, so
    // waiting for it to finish made the client's own request time out while
    // the turn was running perfectly well. `USER_INPUT` and `SEND_MESSAGE` are
    // answered the moment the input is accepted; everything after that arrives
    // on the event stream, which is where the client is already listening.
    let starts_a_turn = match &frame.kind {
        FrameType::Standard(StandardFrameType::UserInput) => true,
        FrameType::Extension(name) if name == crate::NorteCodeExtension::Command.as_str() => {
            serde_json::from_value::<norte_code_wire::ClientRequest>(frame.payload.clone())
                .is_ok_and(|request| {
                    matches!(
                        request.kind,
                        norte_code_wire::ClientRequestKind::SendMessage { .. }
                    )
                })
        }
        _ => false,
    };
    if starts_a_turn {
        let host_for_turn = Arc::clone(&host);
        let host_for_cleanup = Arc::clone(&host);
        let principal_for_turn = principal.clone();
        let key = TurnKey::new(request_session_id.clone(), request_id.clone());
        let cleanup_key = key.clone();
        let handle = tokio::spawn(async move {
            let _ = host_for_turn
                .handle_as(frame, Some(&principal_for_turn))
                .await;
            host_for_cleanup.finish_tracked_turn(&cleanup_key);
        });
        host.track_turn(key, handle);
        return (
            StatusCode::ACCEPTED,
            Json(payload(CommandResponsePayload {
                accepted: true,
                events: Some(Vec::new()),
                error: None,
            })),
        )
            .into_response();
    }
    let result = tokio::time::timeout(
        host.command_timeout(),
        host.handle_as(frame, Some(&principal)),
    )
    .await;
    command_reply(result, request_id, request_session_id)
}

/// Handles a command that is not scoped to a session.
///
/// Catalogue writes, plugin operations, skills and MCP listing belong to the
/// agent, and a client used to reach them through a placeholder session id
/// that existed nowhere. This route takes the frame as it is: a kind that does
/// need a session answers with the service's own error.
pub(super) async fn agent_commands(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    Json(frame): Json<Envelope<Value>>,
) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    let request_id = frame.id.clone();
    let request_session_id = frame.session_id.clone();
    let result = tokio::time::timeout(
        host.command_timeout(),
        host.handle_as(frame, Some(&principal)),
    )
    .await;
    command_reply(result, request_id, request_session_id)
}

fn command_reply(
    result: Result<Result<Vec<Envelope<Value>>, HostError>, tokio::time::error::Elapsed>,
    request_id: raep_types::FrameId,
    request_session_id: String,
) -> Response {
    match result {
        Ok(Ok(events)) => (
            StatusCode::ACCEPTED,
            Json(payload(CommandResponsePayload {
                accepted: true,
                events: Some(events),
                error: None,
            })),
        )
            .into_response(),
        Err(_) => {
            let error = HostError::Service("command timed out".to_owned());
            (
                StatusCode::REQUEST_TIMEOUT,
                Json(correlated_error_frame(
                    &error,
                    Some(request_id),
                    request_session_id,
                )),
            )
                .into_response()
        }
        Ok(Err(error)) => {
            let status = error_status(&error);
            (
                status,
                Json(correlated_error_frame(
                    &error,
                    Some(request_id),
                    request_session_id,
                )),
            )
                .into_response()
        }
    }
}
