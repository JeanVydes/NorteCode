//! Plugin loading for Norte Code.
//!
//! Plugins live in ~/.config/norte-code/plugins/ as dynamic libraries
//! (.so on Linux, .dll on Windows). Each library must export a
//! `create_plugin()` function returning the plugin, and a
//! `norte_plugin_abi_version()` function saying which version of the plugin
//! interface it was built against.
//!
//! The plugins are loaded **once**, at startup, and everybody reads the same
//! registry: a plugin is a stateful object, and loading it again for every
//! command would throw away whatever it kept between one command and the
//! next. It also means that a plugin installed while the client is running is
//! not picked up until it restarts, which is what the client says when one is
//! installed.

use std::sync::{Arc, OnceLock};

use norte_code_agent::Service;
use norte_code_protocol::SessionKey;
use norte_code_protocol::plugins::{PluginContext, PluginHost, Plugins};

/// The plugins this client loaded.
///
/// A static and not a value passed around: the command registry, the sidebar
/// and the commands themselves all need the same plugins, and a registry that
/// was copied would be a copy that lost whatever the plugins kept.
static LOADED: OnceLock<Plugins> = OnceLock::new();

/// Loads the plugins and hands them the host.
///
/// Called once, after the service and the session exist: a plugin that asks
/// the host about the session needs them to be there. Calling it again is
/// harmless — the first call is the one that loads.
pub(crate) fn init(ctx: &PluginContext) -> &'static Plugins {
    LOADED.get_or_init(|| {
        let mut plugins = Plugins::new();
        crate::plugin_loader::load_plugins(&mut plugins, Some(ctx));
        plugins
    })
}

/// The plugins this client loaded.
///
/// Empty when nothing was loaded: a client with no plugins is a client that
/// works, and the registry is not an error.
pub(crate) fn loaded() -> &'static Plugins {
    LOADED.get_or_init(Plugins::new)
}

/// The context the plugins are handed.
///
/// The host is the service behind a trait the plugin can call: a plugin is a
/// separate shared library with its own copy of every crate, so it cannot cast
/// the host's types — the two sides do not share a single one — but it can
/// call them.
pub(crate) fn context(service: &Arc<Service>, session: Option<SessionKey>) -> PluginContext {
    PluginContext {
        session_key: session,
        host: Some(Arc::new(ServiceHost {
            service: Arc::clone(service),
        })),
    }
}

/// The host the plugins ask about the session.
struct ServiceHost {
    service: Arc<Service>,
}

impl PluginHost for ServiceHost {
    fn session_model(&self, session: &SessionKey) -> Option<String> {
        self.service.session_model(session)
    }

    fn session_provider(&self, session: &SessionKey) -> Option<String> {
        self.service.session_spec(session).map(|spec| spec.provider)
    }

    fn session_title(&self, session: &SessionKey) -> Option<String> {
        self.service.session_title(session)
    }
}
