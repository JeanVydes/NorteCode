//! Display lines contributed by plugins loaded at server start.
//!
//! A plugin answers `status()` and `banner()` as plain text. The host captures
//! them once, before anything else asks the plugins, and keeps them beside the
//! name of the plugin that offered them. `PluginInfo` then carries the line as
//! part of the plugin's own metadata, so no second event is needed to paint the
//! sidebar.

use std::collections::BTreeMap;
use std::sync::OnceLock;

#[derive(Clone, Debug, Default)]
struct DisplayLines {
    status: BTreeMap<String, String>,
    banner: BTreeMap<String, String>,
}

static DISPLAY: OnceLock<DisplayLines> = OnceLock::new();

/// Stores the lines the loaded plugins offered.
pub(crate) fn capture(status: Vec<(String, String)>, banner: Vec<(String, String)>) {
    let _ = DISPLAY.set(DisplayLines {
        status: status.into_iter().collect(),
        banner: banner.into_iter().collect(),
    });
}

/// The status line a named plugin offered, when it offered one.
pub(crate) fn status_for(name: &str) -> Option<String> {
    DISPLAY.get()?.status.get(name).cloned()
}

/// The banner line a named plugin offered, when it offered one.
pub(crate) fn banner_for(name: &str) -> Option<String> {
    DISPLAY.get()?.banner.get(name).cloned()
}
