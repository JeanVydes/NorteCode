//! The upload gate refuses anything it cannot verify.

use norte_code_client::ServerInfo;
use norte_code_wire::{AgentRef, Modality, ModelInfo, ReasoningInfo};

use super::{AttachmentError, MimeType, validate};
use crate::ui_model::UiModel;

fn model() -> UiModel {
    let mut model = UiModel::detached();
    let mut info = ServerInfo::default();
    info.capabilities.push("uploads".to_owned());
    model.server_info = Some(info);
    model.active_model = Some(AgentRef {
        id: "vision".to_owned(),
        version: String::new(),
    });
    model.models.push(ModelInfo {
        provider: "test".to_owned(),
        model: "vision".to_owned(),
        name: "Vision".to_owned(),
        description: None,
        max_context: None,
        max_output: None,
        tools: false,
        input: vec![Modality::Image, Modality::File],
        output: vec![Modality::Text],
        reasoning: ReasoningInfo::default(),
        deprecated: false,
    });
    model
}

#[test]
fn an_unknown_model_is_refused() {
    let mut model = model();
    model.active_model = None;
    assert_eq!(
        validate(&model, &MimeType::png()).unwrap_err(),
        AttachmentError::ModelUnknown
    );
}

#[test]
fn a_model_without_image_input_is_refused() {
    let mut model = model();
    model.models[0].input = vec![Modality::File];
    assert!(matches!(
        validate(&model, &MimeType::png()),
        Err(AttachmentError::ModalityRejected { .. })
    ));
}

#[test]
fn json_is_refused_before_any_capability_check() {
    let mut model = model();
    model.server_info = None;
    let mime = MimeType::parse("application/json").expect("valid media type");
    assert!(matches!(
        validate(&model, &mime),
        Err(AttachmentError::UnsupportedMime { .. })
    ));
}

#[test]
fn a_host_without_uploads_is_refused() {
    let mut model = model();
    model.server_info = None;
    assert_eq!(
        validate(&model, &MimeType::png()).unwrap_err(),
        AttachmentError::UploadsUnavailable
    );
}

#[test]
fn a_known_image_model_is_accepted() {
    assert!(validate(&model(), &MimeType::png()).is_ok());
}
