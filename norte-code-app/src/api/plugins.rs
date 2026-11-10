//! One group of endpoints the client exposes.

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::*;

impl ApiClient {
    pub async fn plugins(&self) -> Result<Vec<PluginSummary>, ApiError> {
        let response = self
            .norte_code_command("unused", ClientRequestKind::ListPlugins)
            .await?;
        let payload = response
            .events
            .iter()
            .find(|frame| CanonicalEvent::PluginList.matches(&frame.payload))
            .ok_or_else(|| ApiError::InvalidResponse("Plugin list was not returned.".to_owned()))?;
        Ok(decode_payload::<PluginListResponse>(&payload.payload)?.plugins)
    }

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

    pub async fn update_plugin(&self, name: Option<&str>) -> Result<PluginChange, ApiError> {
        self.plugin_change(ClientRequestKind::UpdatePlugins {
            name: name.map(str::to_owned),
        })
        .await
    }

    pub async fn remove_plugin(&self, name: &str) -> Result<PluginChange, ApiError> {
        self.plugin_change(ClientRequestKind::RemovePlugin {
            name: name.to_owned(),
        })
        .await
    }

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
        let response = self.norte_code_command("unused", command).await?;
        let payload = response
            .events
            .iter()
            .find(|frame| CanonicalEvent::PluginChanged.matches(&frame.payload))
            .ok_or_else(|| {
                ApiError::InvalidResponse("Plugin operation result was not returned.".to_owned())
            })?;
        let change = decode_payload::<PluginChangedResponse>(&payload.payload)?;
        Ok(PluginChange {
            operation: change.operation,
            name: change.name,
        })
    }
}
