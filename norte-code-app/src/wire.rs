use serde::Serialize;

#[derive(Serialize)]
pub struct AppClientRequest {
    pub request_id: u64,
    pub kind: AppRequestKind,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppRequestKind {
    SendMessage {
        session: String,
        text: String,
        attachments: Vec<()>,
        #[serde(skip_serializing_if = "Option::is_none")]
        budget: Option<()>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reasoning: Option<()>,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<()>,
        replay: bool,
    },
}
