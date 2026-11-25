//! Adapter that lets plugins ask the host about the session they run in.

use std::collections::HashMap;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use norte_code_agent::Service;
use norte_code_plugin_host::plugins::PluginHost;
use norte_code_plugin_host::{CommandName, CommandRegistry, Plugins};
use norte_code_wire::{ContextUsage, SessionKey};

#[derive(Default)]
struct AdapterState {
    services: HashMap<String, Arc<Service>>,
    session_agents: HashMap<String, String>,
    current_session: Option<SessionKey>,
    default_agent: Option<String>,
    plugins: Option<Arc<Plugins>>,
    commands: CommandRegistry,
}

/// A plugin host backed by the services the RAEP server owns.
///
/// The adapter exists before the services do, because plugins receive their
/// context while they are being loaded. Calls made before the services are
/// installed answer with what is known, and calls made afterwards answer from
/// the most recently attached session.
pub(crate) struct PluginHostAdapter {
    state: RwLock<AdapterState>,
}

impl PluginHostAdapter {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: RwLock::new(AdapterState::default()),
        })
    }

    pub(crate) fn install_services(
        &self,
        services: HashMap<String, Arc<Service>>,
        default_agent: String,
    ) {
        let mut state = self.write();
        state.services = services;
        state.default_agent = Some(default_agent);
    }

    pub(crate) fn install_plugins(&self, plugins: Arc<Plugins>) {
        let commands = plugins.command_registry();
        let mut state = self.write();
        state.plugins = Some(plugins);
        state.commands = commands;
    }

    pub(crate) fn execute_command(
        &self,
        name: &str,
        arguments: &[String],
    ) -> Result<norte_code_plugin_host::CommandOutput, norte_code_plugin_host::CommandExecutionError>
    {
        let name = CommandName::new(name.to_owned())
            .map_err(|_| norte_code_plugin_host::CommandExecutionError::UnknownCommand)?;
        let state = self.read();
        let plugins = state
            .plugins
            .as_ref()
            .ok_or(norte_code_plugin_host::CommandExecutionError::UnknownCommand)?;
        plugins.execute_command(&state.commands, &name, arguments)
    }

    pub(crate) fn set_session(&self, session: &str, agent: &str) {
        let mut state = self.write();
        state.current_session = Some(SessionKey::new(session));
        state
            .session_agents
            .insert(session.to_owned(), agent.to_owned());
    }

    fn snapshot(&self) -> (Option<SessionKey>, Option<Arc<Service>>) {
        let state = self.read();
        let session = state.current_session.clone();
        let service = session
            .as_ref()
            .and_then(|session| state.session_agents.get(session.as_str()))
            .and_then(|agent| state.services.get(agent))
            .or_else(|| {
                state
                    .default_agent
                    .as_ref()
                    .and_then(|agent| state.services.get(agent))
            })
            .cloned();
        (session, service)
    }

    fn read(&self) -> RwLockReadGuard<'_, AdapterState> {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write(&self) -> RwLockWriteGuard<'_, AdapterState> {
        self.state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl PluginHost for PluginHostAdapter {
    fn session(&self) -> Option<SessionKey> {
        self.snapshot().0
    }

    fn session_model(&self) -> Option<String> {
        let (session, service) = self.snapshot();
        let service = service?;
        match session {
            Some(session) => service.session_spec(&session).map(|spec| spec.model),
            None => service.default_spec().map(|spec| spec.model),
        }
    }

    fn session_provider(&self) -> Option<String> {
        let (session, service) = self.snapshot();
        let service = service?;
        match session {
            Some(session) => service.session_spec(&session).map(|spec| spec.provider),
            None => service.default_spec().map(|spec| spec.provider),
        }
    }

    fn session_title(&self) -> Option<String> {
        let (session, service) = self.snapshot();
        session.and_then(|session| service?.session_title(&session))
    }

    fn session_context(&self) -> ContextUsage {
        let (session, service) = self.snapshot();
        let Some(service) = service else {
            return ContextUsage::default();
        };
        match session {
            Some(session) => service.session_context(&session),
            None => ContextUsage {
                used: 0,
                window: service
                    .default_spec()
                    .and_then(|spec| spec.capabilities.max_context),
            },
        }
    }
}

#[cfg(test)]
#[path = "plugin_host_tests.rs"]
mod tests;
