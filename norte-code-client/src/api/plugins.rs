use norte_code_wire::{CanonicalEvent, ClientRequestKind};

use super::shared::{decode_payload, find_event};
use super::types::{PluginChange, PluginChangedResponse, PluginListResponse, PluginSummary};
use super::{ApiError, RestClient};

impl RestClient {
    /// The installed plugins.
    ///
    /// # Errors
    ///
    /// Returns the transport error or an answer the shape does not describe.
    pub async fn plugins(&self) -> Result<Vec<PluginSummary>, ApiError> {
        let response = self.command(None, ClientRequestKind::ListPlugins).await?;
        let payload = find_event(&response, CanonicalEvent::PluginList)?;
        Ok(decode_payload::<PluginListResponse>(&payload)?.plugins)
    }

    /// Installs a plugin from a repository.
    ///
    /// # Errors
    ///
    /// Returns the transport error or the reason the host refused.
    pub async fn install_plugin(
        &self,
        url: &str,
        package: Option<&str>,
        all: bool,
    ) -> Result<PluginChange, ApiError> {
        self.plugin_change(ClientRequestKind::InstallPlugin {
            url: url.to_owned(),
            package: package.map(str::to_owned),
            all,
        })
        .await
    }

    /// Updates one plugin, or all of them.
    ///
    /// # Errors
    ///
    /// Returns the transport error or the reason the host refused.
    pub async fn update_plugin(&self, name: Option<&str>) -> Result<PluginChange, ApiError> {
        self.plugin_change(ClientRequestKind::UpdatePlugins {
            name: name.map(str::to_owned),
        })
        .await
    }

    /// Removes a plugin.
    ///
    /// # Errors
    ///
    /// Returns the transport error or the reason the host refused.
    pub async fn remove_plugin(&self, name: &str) -> Result<PluginChange, ApiError> {
        self.plugin_change(ClientRequestKind::RemovePlugin {
            name: name.to_owned(),
        })
        .await
    }

    /// Enables or disables a plugin.
    ///
    /// # Errors
    ///
    /// Returns the transport error or the reason the host refused.
    pub async fn set_plugin_enabled(
        &self,
        name: &str,
        enabled: bool,
    ) -> Result<PluginChange, ApiError> {
        self.plugin_change(ClientRequestKind::SetPluginEnabled {
            name: name.to_owned(),
            enabled,
        })
        .await
    }

    async fn plugin_change(&self, command: ClientRequestKind) -> Result<PluginChange, ApiError> {
        let response = self.command(None, command).await?;
        let payload = find_event(&response, CanonicalEvent::PluginChanged)?;
        let change: PluginChangedResponse = decode_payload(&payload)?;
        Ok(PluginChange {
            operation: change.operation,
            name: change.name,
        })
    }
}
