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
//!
//! The plugins are loaded **before** the catalogue is read, because a plugin
//! may offer providers and models of its own: what a plugin offers is in the
//! catalogue for this run, and the files a person owns are not written.
//!
//! The host the plugins ask about the session is a handle that outlives the
//! moment it was made: the service and the session come after the catalogue,
//! and the catalogue comes after the plugins, so a plugin is handed a handle
//! that answers "nothing yet" until the session exists and answers properly
//! afterwards.

use std::sync::{Arc, OnceLock, RwLock};

use norte_code_agent::Service;
use norte_code_protocol::SessionKey;
use norte_code_protocol::plugins::{ContextUsage, PluginContext, PluginHost, Plugins};

/// The plugins this client loaded.
///
/// A static and not a value passed around: the command registry, the sidebar
/// and the commands themselves all need the same plugins, and a registry that
/// was copied would be a copy that lost whatever the plugins kept.
static LOADED: OnceLock<Plugins> = OnceLock::new();

/// The handle the plugins ask about the session.
static HOST: OnceLock<Arc<HostHandle>> = OnceLock::new();

/// Loads the plugins and hands them the host.
///
/// Called once, before the catalogue is read: a plugin that serves a model of
/// its own has to be known before the catalogue is built, or the model would
/// not be in it. Calling it again is harmless — the first call is the one
/// that loads.
pub(crate) fn init() -> &'static Plugins {
    LOADED.get_or_init(|| {
        let mut plugins = Plugins::new();
        // The handle is the plugin's window into the host: it is asked, and
        // it answers, so a plugin never names the host's types.
        let handle = Arc::clone(host());
        let host: Arc<dyn PluginHost> = handle;
        let ctx = PluginContext { host: Some(host) };
        crate::plugin_loader::load_plugins(&mut plugins, Some(&ctx));
        plugins
    })
}

/// The plugins this client loaded.
///
/// The first call loads them and every later one reads the same registry: a
/// plugin is a stateful object, and a registry that was copied would be a copy
/// that lost whatever the plugins kept.
pub(crate) fn loaded() -> &'static Plugins {
    init()
}

/// The handle the plugins ask about the session.
///
/// Created once and filled in later: the plugins are loaded before the
/// service exists, and the handle is what makes that order possible.
fn host() -> &'static Arc<HostHandle> {
    HOST.get_or_init(|| Arc::new(HostHandle::default()))
}

/// Hands the host the service and the session, once they exist.
///
/// The plugins were handed the same handle before, so there is nothing to
/// give them again: what they ask is answered by the handle, and the handle
/// now knows where to look.
pub(crate) fn attach(service: &Arc<Service>, session: Option<SessionKey>) {
    host().set(Arc::clone(service), session);
}

/// What the plugins ask the host about the session.
///
/// The service and the session arrive after this is made, so both are behind
/// a lock and both are optional: a plugin that asks before the session exists
/// is told that there is none, which is the truth.
#[derive(Default)]
struct HostHandle {
    service: RwLock<Option<Arc<Service>>>,
    session: RwLock<Option<SessionKey>>,
}

impl HostHandle {
    /// Remembers the service and the session.
    fn set(&self, service: Arc<Service>, session: Option<SessionKey>) {
        if let Ok(mut slot) = self.service.write() {
            *slot = Some(service);
        }
        if let Ok(mut slot) = self.session.write() {
            *slot = session;
        }
    }

    /// The service, when it exists.
    fn current_service(&self) -> Option<Arc<Service>> {
        self.service.read().ok().and_then(|slot| slot.clone())
    }

    /// The session, when it exists.
    fn current_session(&self) -> Option<SessionKey> {
        self.session.read().ok().and_then(|slot| slot.clone())
    }
}

impl PluginHost for HostHandle {
    fn session(&self) -> Option<SessionKey> {
        self.current_session()
    }

    fn session_model(&self) -> Option<String> {
        let service = self.current_service()?;
        let session = self.current_session()?;
        service.session_model(&session)
    }

    fn session_provider(&self) -> Option<String> {
        let service = self.current_service()?;
        let session = self.current_session()?;
        service.session_spec(&session).map(|spec| spec.provider)
    }

    fn session_title(&self) -> Option<String> {
        let service = self.current_service()?;
        let session = self.current_session()?;
        service.session_title(&session)
    }

    fn session_context(&self) -> ContextUsage {
        let Some(service) = self.current_service() else {
            return ContextUsage::default();
        };
        let Some(session) = self.current_session() else {
            return ContextUsage::default();
        };
        service.session_context(&session)
    }
}
