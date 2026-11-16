use std::sync::Arc;

use super::{ClientData, ClientRuntime};
use crate::components::CatalogOperation;

impl ClientRuntime {
    pub(crate) fn catalog_operation(&self, operation: CatalogOperation, generation: u64) {
        let api = Arc::clone(&self.api);
        self.spawn_data(None, generation, async move {
            match operation {
                CatalogOperation::UpsertProvider { id, provider } => {
                    api.upsert_provider(&id, *provider).await?;
                }
                CatalogOperation::RemoveProvider(id) => {
                    api.remove_provider(&id).await?;
                }
                CatalogOperation::UpsertModel(model) => {
                    api.upsert_model(*model).await?;
                }
                CatalogOperation::RemoveModel { provider, model } => {
                    api.remove_model(&provider, &model).await?;
                }
            }
            api.catalog_detail().await.map(ClientData::Catalog)
        });
    }
}
