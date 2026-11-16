//! Reading the host's live catalogue.
//!
//! The catalogue is what lets the picker tell whether a provider has a known
//! key, and what powers the reasoning-effort guards in the header menus.
use std::sync::Arc;

use crate::client::{ClientData, ClientRuntime};

impl ClientRuntime {
    /// Refreshes the catalogue in the background.
    pub(crate) fn load_catalog(&self, generation: u64) {
        let api = Arc::clone(&self.api);
        self.spawn_data(None, generation, async move {
            api.catalog_detail().await.map(ClientData::Catalog)
        });
    }
}
