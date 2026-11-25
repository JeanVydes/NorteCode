//! HTTP handlers for the RAEP resources.

#[allow(clippy::wildcard_imports)]
use super::*;

pub(super) async fn list_agents(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = principal(&host, &headers) {
        return response;
    }
    Json(payload(AgentListPayload {
        agents: host.agents(),
        next_cursor: Value::Null,
    }))
    .into_response()
}

pub(super) async fn read_agent(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    Path(agent_id): Path<String>,
) -> Response {
    if let Err(response) = principal(&host, &headers) {
        return response;
    }
    match host.agent(&agent_id) {
        Some(agent) => Json(agent).into_response(),
        None => not_found_message("agent not found"),
    }
}

pub(super) async fn agent_versions(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    Path(agent_id): Path<String>,
) -> Response {
    if let Err(response) = principal(&host, &headers) {
        return response;
    }
    match host.agent_versions(&agent_id) {
        Some(versions) => Json(versions).into_response(),
        None => not_found_message("agent not found"),
    }
}

pub(super) async fn list_sessions(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    Json(payload(SessionListPayload {
        sessions: host.session_summaries(&principal).await,
        next_cursor: Value::Null,
    }))
    .into_response()
}

pub(super) async fn list_child_sessions(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    match host.child_sessions(&session_id, &principal).await {
        Some(children) => Json(payload(ChildSessionListPayload {
            children: children
                .into_iter()
                .map(|child| ChildSessionPayload {
                    session_id: child.session.to_string(),
                    title: child.title,
                    state: child.state,
                    ..ChildSessionPayload::default()
                })
                .collect(),
        }))
        .into_response(),
        None => not_found(),
    }
}

pub(super) async fn create_session(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    let body = body.map_or(Value::Null, |Json(value)| value);
    let agent_id = body
        .get("agent_id")
        .and_then(Value::as_str)
        .filter(|agent| !agent.is_empty())
        .unwrap_or_else(|| host.agent_id());
    if host.agent(agent_id).is_none() {
        return invalid(&format!("unknown agent `{agent_id}`"));
    }
    if body.get("labels").is_some_and(|value| !value.is_null()) {
        return invalid("labels are not supported by this host");
    }
    let idempotency = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    if let Some(key) = &idempotency
        && let Some(session) = host.session_for_create_key(key).await
    {
        return match host.session_summary(&session, &principal).await {
            Some(summary) => (StatusCode::OK, Json(summary)).into_response(),
            None => not_found(),
        };
    }
    let title = body.get("title").and_then(Value::as_str).map(str::to_owned);
    let agent_override = body.get("agent_override").filter(|value| !value.is_null());
    let session = match host
        .create_session(agent_id, title, agent_override, &principal)
        .await
    {
        Ok(session) => session,
        Err(error) => return invalid(&error.to_string()),
    };
    if let Some(key) = &idempotency {
        host.remember_create_key(key, &session).await;
    }
    match host.session_summary(&session, &principal).await {
        Some(summary) => (StatusCode::CREATED, Json(summary)).into_response(),
        None => invalid("created session could not be read back"),
    }
}

pub(super) async fn read_session(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    match host.session_summary(&session_id, &principal).await {
        Some(summary) => Json(summary).into_response(),
        None => not_found(),
    }
}

pub(super) async fn patch_session(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    body: Option<Json<Value>>,
) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    let body = body.map_or(Value::Null, |Json(value)| value);
    let Some(object) = body.as_object() else {
        return invalid("PATCH body must be a JSON object");
    };
    let mut title = None;
    for (key, value) in object {
        match key.as_str() {
            "title" => {
                let Some(text) = value.as_str() else {
                    return invalid("title must be a string");
                };
                title = Some(text.to_owned());
            }
            "labels" | "pinned" | "archived" => {
                if !value.is_null() {
                    return invalid(&format!("{key} is not supported by this host"));
                }
            }
            other => return invalid(&format!("unknown field `{other}`")),
        }
    }
    let Some(title) = title else {
        return invalid("PATCH must set the title");
    };
    match host.rename_session(&session_id, &title, &principal).await {
        Ok(true) => match host.session_summary(&session_id, &principal).await {
            Some(summary) => Json(summary).into_response(),
            None => not_found(),
        },
        Ok(false) => not_found(),
        Err(error) => invalid(&error.to_string()),
    }
}

pub(super) async fn delete_session(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    match host.delete_session(&session_id, &principal).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => not_found(),
        Err(error) => invalid(&error.to_string()),
    }
}
