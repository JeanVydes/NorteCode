use norte_code_wire::{ClientRequest, ClientRequestKind, SessionKey};

use super::ApiClient;
use serde_json::json;

#[test]
fn session_url_has_no_empty_path_segment() {
    let client = ApiClient::new("http://127.0.0.1:8787", "").expect("valid server URL");
    let url = client
        .session_url("00fae2ff-5474-482b-9751-6d11b084345f", "commands")
        .expect("valid session URL");

    assert_eq!(
        url.as_str(),
        "http://127.0.0.1:8787/raep/v1/sessions/00fae2ff-5474-482b-9751-6d11b084345f/commands"
    );
}

#[test]
fn session_url_preserves_server_base_path() {
    let client = ApiClient::new("https://example.test/raep-proxy/", "").expect("valid server URL");
    let url = client
        .session_url("session-id", "uploads")
        .expect("valid session URL");

    assert_eq!(
        url.as_str(),
        "https://example.test/raep-proxy/raep/v1/sessions/session-id/uploads"
    );
}

#[test]
fn session_resource_url_encodes_the_server_supplied_id_as_one_segment() {
    let client = ApiClient::new("http://127.0.0.1:8787", "").expect("valid server URL");
    let url = client
        .session_resource_url("session/with space")
        .expect("valid session resource URL");

    assert_eq!(
        url.as_str(),
        "http://127.0.0.1:8787/raep/v1/sessions/session%2Fwith%20space"
    );
}

#[test]
fn session_model_request_matches_the_host_extension_wire() {
    let request = ClientRequest {
        request_id: 7,
        kind: ClientRequestKind::SetModel {
            session: SessionKey::new("session-1"),
            provider: "provider-1".to_owned(),
            model: "model-1".to_owned(),
        },
    };

    assert_eq!(
        serde_json::to_value(request).expect("serializable request"),
        json!({
            "request_id": 7,
            "kind": {
                "type": "set_model",
                "session": "session-1",
                "provider": "provider-1",
                "model": "model-1"
            }
        })
    );
}

#[test]
fn answer_requests_use_the_host_supported_command_name() {
    let answer = serde_json::to_value(ClientRequestKind::AnswerQuestion {
        session: SessionKey::new("session-1"),
        question: "question-1".to_owned(),
        answer: "continue".to_owned(),
    })
    .expect("serializable answer");

    assert_eq!(
        answer,
        json!({
            "type": "answer_question",
            "session": "session-1",
            "question": "question-1",
            "answer": "continue"
        })
    );
}
