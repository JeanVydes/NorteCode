//! One group of endpoints the client exposes.

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::*;

impl ApiClient {
    pub async fn models(&self) -> Result<Vec<ModelSummary>, ApiError> {
        let response = self
            .norte_code_command("unused", ClientRequestKind::ListModels)
            .await?;
        let payload = response
            .events
            .iter()
            .find(|frame| CanonicalEvent::agent_models().matches(&frame.payload))
            .ok_or_else(|| {
                ApiError::InvalidResponse("Model catalogue was not returned.".to_owned())
            })?;
        Ok(decode_payload::<AgentCatalogResponse>(&payload.payload)?.entries)
    }

    /// The catalogue with its origin, for the editor.
    pub async fn catalog_detail(&self) -> Result<CatalogDetail, ApiError> {
        let response = self
            .norte_code_command("unused", ClientRequestKind::ListCatalog)
            .await?;
        let payload = response
            .events
            .iter()
            .find(|frame| CanonicalEvent::catalog_providers().matches(&frame.payload))
            .ok_or_else(|| {
                ApiError::InvalidResponse("The catalogue detail was not returned.".to_owned())
            })?;
        // The providers catalogue is an `agent_catalog` envelope: the entries
        // carry both lists, under `entries`, not at the top level.
        decode_payload(&payload.payload["entries"])
    }

    /// Writes one provider into `providers.json` and reloads the catalogue.
    pub async fn upsert_provider(
        &self,
        provider_id: &str,
        provider: norte_code_wire::catalog::Provider,
    ) -> Result<(), ApiError> {
        self.norte_code_command(
            "unused",
            ClientRequestKind::CatalogUpsertProvider {
                provider_id: provider_id.to_owned(),
                provider,
            },
        )
        .await
        .map(|_| ())
    }

    /// Removes a provider and its models from the catalogue files.
    pub async fn remove_provider(&self, provider_id: &str) -> Result<(), ApiError> {
        self.norte_code_command(
            "unused",
            ClientRequestKind::CatalogRemoveProvider {
                provider_id: provider_id.to_owned(),
            },
        )
        .await
        .map(|_| ())
    }

    /// Writes one model into `models.json`.
    pub async fn upsert_model(
        &self,
        model: norte_code_wire::catalog::Model,
    ) -> Result<(), ApiError> {
        self.norte_code_command("unused", ClientRequestKind::CatalogUpsertModel { model })
            .await
            .map(|_| ())
    }

    /// Removes one model from `models.json`.
    pub async fn remove_model(&self, provider_id: &str, model_id: &str) -> Result<(), ApiError> {
        self.norte_code_command(
            "unused",
            ClientRequestKind::CatalogRemoveModel {
                provider_id: provider_id.to_owned(),
                model_id: model_id.to_owned(),
            },
        )
        .await
        .map(|_| ())
    }
}
