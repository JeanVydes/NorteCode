//! Shared plugin actions for the settings page.

use serde::Serialize;

/// A pending request to the app layer, drained each frame.
#[derive(Serialize)]
pub(crate) enum PluginsAction {
    /// Install a repository, optionally one package.
    Install {
        /// The repository URL.
        url: String,
        /// One workspace package, or the whole workspace.
        package: Option<String>,
    },
    /// Update one plugin, or all when `name` is `None`.
    Update {
        /// The plugin, or every plugin when missing.
        name: Option<String>,
    },
    /// Remove one plugin.
    Remove {
        /// The plugin to remove.
        name: String,
    },
    /// Enable or disable one plugin.
    SetEnabled {
        /// The plugin to toggle.
        name: String,
        /// What it should be after the change.
        enabled: bool,
    },
}
