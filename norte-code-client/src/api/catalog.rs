use norte_code_wire::{CanonicalEvent, ClientRequestKind};

use super::shared::{decode_payload, find_event};
use super::types::{AgentCatalogResponse, CatalogDetail};
use super::{ApiError, RestClient};

impl RestClient {
    /// The model catalogue.
    ///
    /// # Errors
    ///
    /// Returns the transport error or an answer the shape does not describe.
    pub async fn models(&self) -> Result<Vec<norte_code_wire::ModelInfo>, ApiError> {
        let response = self.command(None, ClientRequestKind::ListModels).await?;
        let payload = find_event(&response, CanonicalEvent::agent_models())?;
        Ok(decode_payload::<AgentCatalogResponse>(&payload)?.entries)
    }

    /// The editable catalogue with where each entry came from.
    ///
    /// # Errors
    ///
    /// Returns the transport error or an answer the shape does not describe.
    pub async fn catalog_detail(&self) -> Result<CatalogDetail, ApiError> {
        let response = self.command(None, ClientRequestKind::ListCatalog).await?;
        let payload = find_event(&response, CanonicalEvent::catalog_providers())?;
        // The providers catalogue is an `agent_catalog` envelope: the entries
        // carry both lists, under `entries`, not at the top level.
        decode_payload(&payload["entries"])
    }

    /// Writes one provider into `providers.json` and reloads the catalogue.
    ///
    /// # Errors
    ///
    /// Returns the transport error or the reason the host refused.
    pub async fn upsert_provider(
        &self,
        provider_id: &str,
        provider: norte_code_wire::catalog::Provider,
    ) -> Result<(), ApiError> {
        self.command_ok(
            None,
            ClientRequestKind::CatalogUpsertProvider {
                provider_id: provider_id.to_owned(),
                provider,
            },
        )
        .await
    }

    /// Removes a provider and its models from the catalogue files.
    ///
    /// # Errors
    ///
    /// Returns the transport error or the reason the host refused.
    pub async fn remove_provider(&self, provider_id: &str) -> Result<(), ApiError> {
        self.command_ok(
            None,
            ClientRequestKind::CatalogRemoveProvider {
                provider_id: provider_id.to_owned(),
            },
        )
        .await
    }

    /// Writes one model into `models.json`.
    ///
    /// # Errors
    ///
    /// Returns the transport error or the reason the host refused.
    pub async fn upsert_model(
        &self,
        model: norte_code_wire::catalog::Model,
    ) -> Result<(), ApiError> {
        self.command_ok(None, ClientRequestKind::CatalogUpsertModel { model })
            .await
    }

    /// Removes one model from `models.json`.
    ///
    /// # Errors
    ///
    /// Returns the transport error or the reason the host refused.
    pub async fn remove_model(&self, provider_id: &str, model_id: &str) -> Result<(), ApiError> {
        self.command_ok(
            None,
            ClientRequestKind::CatalogRemoveModel {
                provider_id: provider_id.to_owned(),
                model_id: model_id.to_owned(),
            },
        )
        .await
    }
}
