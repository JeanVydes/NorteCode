//! Remote plugin compatibility hooks.

use std::sync::OnceLock;

use norte_code_protocol::SessionKey;
use norte_code_protocol::plugins::Plugins;

static PLUGINS: OnceLock<Plugins> = OnceLock::new();

/// Returns an empty local registry.
///
/// Plugin loading and execution belong to the RAEP server. The empty registry
/// keeps the command parser compatible until remote plugin command events are
/// rendered by the client.
pub(crate) fn init() -> &'static Plugins {
    PLUGINS.get_or_init(Plugins::new)
}

/// Returns the empty local compatibility registry.
pub(crate) fn loaded() -> &'static Plugins {
    init()
}

/// Records no local session state because the plugin host is remote.
pub(crate) fn attach(_session: Option<SessionKey>) {}
