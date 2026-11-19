use std::sync::Arc;

use super::{ClientData, ClientRuntime};
use crate::components::PluginOperation;

impl ClientRuntime {
    pub(crate) fn load_plugins(&self, generation: u64) {
        let api = Arc::clone(&self.api);
        self.spawn_data(None, generation, async move {
            api.plugins().await.map(|plugins| ClientData::PluginUpdate {
                change: norte_code_client::PluginChange::default(),
                plugins,
            })
        });
    }

    pub(crate) fn plugin_operation(&self, operation: PluginOperation, generation: u64) {
        let api = Arc::clone(&self.api);
        self.spawn_data(None, generation, async move {
            let change = match operation {
                PluginOperation::Install { url, package } => {
                    api.install_plugin(&url, package.as_deref(), false).await?
                }
                PluginOperation::Update { name } => api.update_plugin(name.as_deref()).await?,
                PluginOperation::Remove { name } => api.remove_plugin(&name).await?,
                PluginOperation::Enable { name } => api.set_plugin_enabled(&name, true).await?,
                PluginOperation::Disable { name } => api.set_plugin_enabled(&name, false).await?,
            };
            let plugins = api.plugins().await?;
            Ok(ClientData::PluginUpdate { change, plugins })
        });
    }
}
