//! HTTP handlers for the RAEP artifacts.

#[allow(clippy::wildcard_imports)]
use super::*;

pub(super) async fn upload_artifact(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };

    let mime_type = headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream");
    match host
        .put_artifact_as(mime_type, &body, Some(&principal))
        .await
    {
        Ok(reference) => Json(payload(ArtifactRefPayload {
            id: &reference.id,
            mime_type: &reference.mime_type,
            bytes: reference.bytes,
        }))
        .into_response(),
        Err(error) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(payload(RejectedPayload {
                status: "error",
                accepted: Some(false),
                error: ErrorBody {
                    code: "ARTIFACT_FAILED",
                    message: error.to_string(),
                },
            })),
        )
            .into_response(),
    }
}

pub(super) async fn upload_session_upload(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    body: Bytes,
) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    if host
        .session_summary(&session_id, &principal)
        .await
        .is_none()
    {
        return not_found();
    }
    let Some(mime_type) = headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
    else {
        return invalid("Content-Type is required for uploads");
    };
    if mime_type == "application/json" {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(payload(RejectedPayload {
                status: "error",
                accepted: Some(false),
                error: ErrorBody {
                    code: "UPLOAD_MODE_UNSUPPORTED",
                    message: "resumable uploads are not supported".to_owned(),
                },
            })),
        )
            .into_response();
    }
    match host
        .put_session_upload_as(&session_id, mime_type, &body, Some(&principal))
        .await
    {
        Ok(receipt) => (StatusCode::CREATED, Json(receipt)).into_response(),
        Err(error) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(payload(RejectedPayload {
                status: "error",
                accepted: Some(false),
                error: ErrorBody {
                    code: "UPLOAD_REJECTED",
                    message: error.to_string(),
                },
            })),
        )
            .into_response(),
    }
}

pub(super) async fn download_session_output(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    Path((session_id, output_id)): Path<(String, String)>,
) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    let Ok((bytes, mime_type)) = host
        .get_session_output_as(&session_id, &output_id, Some(&principal))
        .await
    else {
        return not_found();
    };
    let range = match headers
        .get(axum::http::header::RANGE)
        .and_then(|value| value.to_str().ok())
    {
        Some(value) => match parse_byte_range(value, bytes.len()) {
            Ok(range) => range,
            Err(()) => {
                return (
                    StatusCode::RANGE_NOT_SATISFIABLE,
                    [
                        ("accept-ranges", "bytes".to_owned()),
                        ("content-range", format!("bytes */{}", bytes.len())),
                    ],
                )
                    .into_response();
            }
        },
        None => None,
    };
    let Ok(content_type) = HeaderValue::from_str(&mime_type) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let mut response_headers = HeaderMap::new();
    response_headers.insert(axum::http::header::CONTENT_TYPE, content_type);
    response_headers.insert(
        axum::http::header::ACCEPT_RANGES,
        HeaderValue::from_static("bytes"),
    );
    match range {
        Some((start, end)) => {
            if let Ok(value) =
                HeaderValue::from_str(&format!("bytes {start}-{end}/{}", bytes.len()))
            {
                response_headers.insert(axum::http::header::CONTENT_RANGE, value);
            }
            (
                StatusCode::PARTIAL_CONTENT,
                response_headers,
                Bytes::copy_from_slice(&bytes[start..=end]),
            )
                .into_response()
        }
        None => (StatusCode::OK, response_headers, Bytes::from(bytes)).into_response(),
    }
}

pub(super) async fn download_artifact(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    Path(artifact_id): Path<String>,
) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    match host.get_artifact_as(&artifact_id, Some(&principal)).await {
        Ok(bytes) => (
            [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
            bytes,
        )
            .into_response(),
        Err(error) => (
            StatusCode::NOT_FOUND,
            Json(payload(RejectedPayload {
                status: "error",
                accepted: Some(false),
                error: ErrorBody {
                    code: "ARTIFACT_NOT_FOUND",
                    message: error.to_string(),
                },
            })),
        )
            .into_response(),
    }
}

pub(super) async fn download_export(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    Path(export_id): Path<String>,
) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    let Ok((bytes, _)) = host.get_export_as(&export_id, &principal).await else {
        return not_found();
    };
    let range = match headers
        .get(axum::http::header::RANGE)
        .and_then(|value| value.to_str().ok())
    {
        Some(value) => match parse_byte_range(value, bytes.len()) {
            Ok(range) => range,
            Err(()) => {
                return (
                    StatusCode::RANGE_NOT_SATISFIABLE,
                    [
                        ("accept-ranges", "bytes".to_owned()),
                        ("content-range", format!("bytes */{}", bytes.len())),
                    ],
                )
                    .into_response();
            }
        },
        None => None,
    };
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/vnd.raep.bundle+zip"),
    );
    response_headers.insert(
        axum::http::header::ACCEPT_RANGES,
        HeaderValue::from_static("bytes"),
    );
    match range {
        Some((start, end)) => {
            if let Ok(value) =
                HeaderValue::from_str(&format!("bytes {start}-{end}/{}", bytes.len()))
            {
                response_headers.insert(axum::http::header::CONTENT_RANGE, value);
            }
            (
                StatusCode::PARTIAL_CONTENT,
                response_headers,
                Bytes::copy_from_slice(&bytes[start..=end]),
            )
                .into_response()
        }
        None => (StatusCode::OK, response_headers, Bytes::from(bytes)).into_response(),
    }
}

pub(super) async fn import_bundle(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    match host.preview_import_as(&body, &principal).await {
        Ok(report) => (StatusCode::CREATED, Json(report)).into_response(),
        Err(error) => invalid(&error.to_string()),
    }
}

pub(super) async fn commit_import(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    Path(import_id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    let request = match serde_json::from_value::<crate::host::ImportCommitRequest>(body) {
        Ok(request) => request,
        Err(error) => return invalid(&error.to_string()),
    };
    match host.commit_import_as(&import_id, request, &principal).await {
        Ok(result) => Json(result).into_response(),
        Err(crate::host::HostError::InvalidFrame(message)) if message == "IMPORT_NOT_FOUND" => {
            not_found_message("import not found or expired")
        }
        Err(error) => invalid(&error.to_string()),
    }
}

pub(super) async fn delete_import(
    State(host): State<Arc<NorteCodeRaepHost>>,
    headers: HeaderMap,
    Path(import_id): Path<String>,
) -> Response {
    let principal = match principal(&host, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    if host.delete_import_as(&import_id, &principal).await {
        StatusCode::NO_CONTENT.into_response()
    } else {
        not_found_message("import not found or expired")
    }
}

pub(crate) fn parse_byte_range(value: &str, total: usize) -> Result<Option<(usize, usize)>, ()> {
    let Some(range) = value.strip_prefix("bytes=") else {
        return Err(());
    };
    if range.contains(',') || total == 0 {
        return Err(());
    }
    let (start, end) = range.split_once('-').ok_or(())?;
    let (start, end) = if start.is_empty() {
        let suffix = end.parse::<usize>().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        (total.saturating_sub(suffix), total - 1)
    } else {
        let start = start.parse::<usize>().map_err(|_| ())?;
        let end = if end.is_empty() {
            total - 1
        } else {
            end.parse::<usize>().map_err(|_| ())?.min(total - 1)
        };
        (start, end)
    };
    if start >= total || start > end {
        return Err(());
    }
    Ok(Some((start, end)))
}
