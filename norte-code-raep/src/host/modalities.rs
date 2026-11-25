//! Input modality negotiation and type mapping.

use norte_code_agent::Service;
use norte_code_wire::SessionKey;

pub(crate) fn supported_input_modalities(
    service: &Service,
    session: &SessionKey,
) -> Vec<norte::Modality> {
    let Some(spec) = service.session_spec(session) else {
        return Vec::new();
    };
    [
        norte::Modality::Text,
        norte::Modality::Image,
        norte::Modality::Document,
        norte::Modality::Audio,
        norte::Modality::Video,
    ]
    .into_iter()
    .filter(|modality| spec.capabilities.modalities.supports_input(*modality))
    .collect()
}

pub(crate) fn to_norte_modality(modality: &raep_types::Modality) -> norte::Modality {
    match modality {
        raep_types::Modality::Text => norte::Modality::Text,
        raep_types::Modality::Image => norte::Modality::Image,
        raep_types::Modality::Document => norte::Modality::Document,
        raep_types::Modality::Audio => norte::Modality::Audio,
        raep_types::Modality::Video => norte::Modality::Video,
    }
}

pub(crate) fn raep_modality_for_media(kind: norte_code_wire::MediaKind) -> raep_types::Modality {
    match kind {
        norte_code_wire::MediaKind::Image => raep_types::Modality::Image,
        norte_code_wire::MediaKind::Document => raep_types::Modality::Document,
        norte_code_wire::MediaKind::Audio => raep_types::Modality::Audio,
        norte_code_wire::MediaKind::Video => raep_types::Modality::Video,
    }
}
