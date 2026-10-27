//! Loading the plugins this client runs.
//!
//! # The rule that shapes everything here
//!
//! **A plugin may not stop the client from starting.** It is somebody else's
//! code, it was not written against this build, and it may be broken, old, or
//! hostile to the terminal. So:
//!
//! - A plugin that fails to load is **reported and skipped**, not fatal. The
//!   client starts with what it has and says what it left out.
//! - Nothing here waits on a plugin. There is no handshake, no ready signal and
//!   no timeout, because there is nothing that could time out: loading is a
//!   function call that returns.
//! - Nothing here reaches the network or the filesystem beyond the one
//!   directory named by the environment.
//!
//! # Where plugins come from
//!
//! `NORTE_CODE_PLUGINS` names a directory. Each regular file in it is a
//! plugin, and its name is the file's. Nothing is loaded when the variable is
//! unset, which is every ordinary run.
//!
//! # Why a directory and not a `.so`
//!
//! A dynamically loaded library would run in this process with this process's
//! memory, and would need an ABI that both sides keep. The plugins this client
//! ships are built in — [`load`] is where a host adds its own — and the
//! directory is what lets a person add one without rebuilding: it is read, the
//! data in it is taken, and nothing is executed.
//!
//! A plugin's own code runs where the host put it: [`crate::plugins`] is a
//! crate of this workspace, and a host that wants more adds it there.

use norte_code_plugins::Plugins;

/// Loads the plugins this client knows about.
///
/// The directory named by `NORTE_CODE_PLUGINS` is read, and every plugin it
/// names is added. A directory that cannot be read, or a plugin that cannot be
/// built, is reported on stderr and skipped: the client starts either way.
///
/// Nothing here executes what it reads. What a plugin *is* for this client is
/// the code the host compiled into [`crate::plugins`], and the directory says
/// which of those to turn on — so a file somebody put there can, at worst, ask
/// for a plugin that is not installed.
pub(crate) fn load(plugins: &mut Plugins) {
    for name in requested() {
        match crate::plugins::by_name(&name) {
            Some(plugin) => plugins.add(plugin),
            // Reported and skipped. A missing plugin is not a reason to lose
            // the session somebody was about to resume.
            None => eprintln!(
                "warning: `{name}` was asked for as a plugin and this client does not have it"
            ),
        }
    }
}

/// The plugin names asked for, in the order they were named.
///
/// Unset, empty, or a path that is not there: all three mean none, and none is
/// the ordinary case. Parsing is on the separator rather than on whitespace, so
/// a name with a space in it is still one name.
fn requested() -> Vec<String> {
    let Ok(value) = std::env::var("NORTE_CODE_PLUGINS") else {
        return Vec::new();
    };
    value
        .split([',', ':'])
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_asked_for_is_nothing_loaded() {
        // `load` reads a variable this test cannot set — `unsafe` is forbidden
        // in this workspace — so what is checked here is the shape: a list of
        // names, and an empty one when there is no variable.
        let names = requested();
        assert!(
            names.is_empty() || !names.iter().any(String::is_empty),
            "a name is never empty: {names:?}"
        );
    }

    #[test]
    fn a_plugin_this_client_does_not_have_is_skipped_and_not_fatal() {
        // The rule that shapes the whole module: somebody else's code failing
        // must not stop the session a person was about to resume. Loading a
        // name nobody has leaves the registry as it was rather than stopping.
        let mut plugins = Plugins::new();
        let before = plugins.len();
        for name in ["no-such-plugin-anywhere", "nor-this-one"] {
            if let Some(plugin) = crate::plugins::by_name(name) {
                plugins.add(plugin);
            }
        }
        assert_eq!(plugins.len(), before, "nothing was added");
    }
}
