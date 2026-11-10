//! One of the service's domains, as `impl Service` blocks.
//!
//! The parent's imports are in scope through `super`, so the domain
//! names types the same way the service does.

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::*;

impl Service {
    pub(crate) fn lock_catalog(&self) -> RwLockReadGuard<'_, Arc<Catalog>> {
        self.catalog.read().unwrap_or_else(PoisonError::into_inner)
    }
    pub(crate) fn lock_catalog_write(&self) -> RwLockWriteGuard<'_, Arc<Catalog>> {
        self.catalog.write().unwrap_or_else(PoisonError::into_inner)
    }
    pub(crate) fn lock_plugin_catalogue(&self) -> RwLockReadGuard<'_, (Providers, Models)> {
        self.plugin_catalogue
            .read()
            .unwrap_or_else(PoisonError::into_inner)
    }
    /// The plugin catalogue folded over the catalogue on disk.
    ///
    /// A plugin offers providers and models that `providers.json` and
    /// `models.json` do not carry, and the host calls
    /// [`Service::set_plugin_catalogue`] before any session exists. A resolve
    /// that only looked at the files would then say a plugin-provided pair is
    /// unknown; this is the one place the two sources are joined, and a
    /// resolve tries it when the file catalogue misses.
    pub(crate) fn catalog_with_plugins(&self) -> Arc<Catalog> {
        let catalog = self.catalog();
        let (providers, models) = self.lock_plugin_catalogue().clone();
        if providers.providers.is_empty() && models.models.is_empty() {
            return catalog;
        }
        let mut merged = (*catalog).clone();
        for skipped in merged.extend(providers.providers, models.models) {
            tracing::debug!("plugin catalogue entry not applied: {skipped}");
        }
        Arc::new(merged)
    }
    pub(crate) fn lock_plugin_catalogue_write(&self) -> RwLockWriteGuard<'_, (Providers, Models)> {
        self.plugin_catalogue
            .write()
            .unwrap_or_else(PoisonError::into_inner)
    }
    pub(crate) fn lock_skills(&self) -> RwLockReadGuard<'_, Arc<SkillRegistry>> {
        self.skills.read().unwrap_or_else(PoisonError::into_inner)
    }
    pub(crate) fn lock_skills_write(&self) -> RwLockWriteGuard<'_, Arc<SkillRegistry>> {
        self.skills.write().unwrap_or_else(PoisonError::into_inner)
    }
}
