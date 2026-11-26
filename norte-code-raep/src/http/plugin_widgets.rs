use super::{
    Arc, HeaderMap, IntoResponse, Json, NorteCodeRaepHost, Response, State, StatusCode, principal,
};

use norte_code_wire::plugin_widgets::PluginWidgetRequest;

pub(super) async fn read(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    Json(request): Json<PluginWidgetRequest>,
) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    if let Some(session) = &request.selection.session
        && host
            .session_summary(session.as_str(), &principal)
            .await
            .is_none()
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(plugins) = &host.plugin_host else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let service = request
        .selection
        .session
        .as_ref()
        .and_then(|session| host.service_for_session(session.as_str()).ok());
    Json(plugins.widgets(request, service).await).into_response()
}
