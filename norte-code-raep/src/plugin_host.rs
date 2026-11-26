//! Loaded plugins and scoped widget invalidations.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use norte_code_plugin_host::{
    CommandExecutionError, CommandName, CommandOutput, CommandRegistry, PluginContext,
    PluginInvocation, Plugins,
};
use norte_code_wire::plugin_widgets::{
    PluginSelection, PluginWidgetRequest, PluginWidgetSnapshot, Widget, WidgetKey,
    WidgetRegistration, WidgetUpdate,
};
use tokio::sync::watch;

const MAX_SUBSCRIBED_WIDGETS: usize = 64;
const MAX_WIDGET_RESPONSE_BYTES: usize = 64 * 1024;

struct Registry {
    plugins: Arc<Plugins>,
    commands: CommandRegistry,
}

#[derive(Default)]
struct WidgetRevisions {
    current: u64,
    by_widget: BTreeMap<WidgetKey, u64>,
}

impl WidgetRevisions {
    fn invalidate(&mut self, key: &WidgetKey) -> bool {
        let Some(revision) = self.by_widget.get_mut(key) else {
            return false;
        };
        self.current = self.current.wrapping_add(1);
        *revision = self.current;
        true
    }

    fn latest(&self, keys: &[WidgetKey]) -> u64 {
        keys.iter()
            .filter_map(|key| self.by_widget.get(key).copied())
            .max()
            .unwrap_or_default()
    }
}

pub(crate) struct PluginHostAdapter {
    registry: OnceLock<Registry>,
    revisions: Arc<watch::Sender<WidgetRevisions>>,
}

impl PluginHostAdapter {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            registry: OnceLock::new(),
            revisions: Arc::new(watch::Sender::new(WidgetRevisions::default())),
        })
    }

    pub(crate) fn context(&self, plugin: &str) -> PluginContext {
        let revisions = Arc::clone(&self.revisions);
        let plugin = plugin.to_owned();
        PluginContext::new(move |id| {
            let key = WidgetKey {
                plugin: plugin.clone(),
                widget: id.clone(),
            };
            revisions.send_if_modified(|state| state.invalidate(&key));
        })
    }

    pub(crate) fn install_plugins(&self, plugins: Arc<Plugins>, widgets: &[WidgetRegistration]) {
        self.revisions.send_modify(|revisions| {
            revisions.by_widget = widgets.iter().map(|entry| (entry.key.clone(), 0)).collect();
        });
        let _ = self.registry.set(Registry {
            commands: plugins.command_registry(),
            plugins,
        });
    }

    pub(crate) async fn widgets(
        &self,
        mut request: PluginWidgetRequest,
        session_service: Option<Arc<norte_code_agent::Service>>,
    ) -> PluginWidgetSnapshot {
        request.widgets.truncate(MAX_SUBSCRIBED_WIDGETS);
        let mut revisions = self.revisions.subscribe();
        {
            let state = revisions.borrow();
            let mut seen = std::collections::BTreeSet::new();
            request
                .widgets
                .retain(|key| state.by_widget.contains_key(key) && seen.insert(key.clone()));
        }
        let revision = wait_for_revision(&mut revisions, &request).await;
        let updates = if let Some(registry) = self.registry.get() {
            let plugins = Arc::clone(&registry.plugins);
            tokio::task::spawn_blocking(move || {
                widget_updates(&plugins, request, session_service.as_deref())
            })
            .await
            .unwrap_or_default()
        } else {
            Vec::new()
        };
        PluginWidgetSnapshot { revision, updates }
    }

    pub(crate) fn execute_command(
        &self,
        name: &str,
        arguments: &[String],
        invocation: Option<&PluginInvocation>,
    ) -> Result<CommandOutput, CommandExecutionError> {
        let name =
            CommandName::new(name.to_owned()).map_err(|_| CommandExecutionError::UnknownCommand)?;
        let registry = self
            .registry
            .get()
            .ok_or(CommandExecutionError::UnknownCommand)?;
        registry
            .plugins
            .execute_command(&registry.commands, &name, arguments, invocation)
    }
}

async fn wait_for_revision(
    revisions: &mut watch::Receiver<WidgetRevisions>,
    request: &PluginWidgetRequest,
) -> u64 {
    let _ = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if request.after != Some(revisions.borrow_and_update().latest(&request.widgets))
                || revisions.changed().await.is_err()
            {
                break;
            }
        }
    })
    .await;
    // Read the revision before asking the plugins. An update that races with
    // evaluation will remain visible to the client's next request.
    revisions.borrow().latest(&request.widgets)
}

pub(crate) fn invocation(
    selection: PluginSelection,
    service: Option<&norte_code_agent::Service>,
) -> PluginInvocation {
    let current = service.zip(selection.session.as_ref());
    let title = current.and_then(|(service, session)| service.session_title(session));
    let context_usage = current.map_or_else(
        norte_code_wire::ContextUsage::default,
        |(service, session)| service.session_context(session),
    );
    PluginInvocation {
        selection,
        title,
        context_usage,
    }
}

fn widget_updates(
    plugins: &Plugins,
    request: PluginWidgetRequest,
    service: Option<&norte_code_agent::Service>,
) -> Vec<WidgetUpdate> {
    let invocation = invocation(request.selection, service);
    let mut bytes_left = MAX_WIDGET_RESPONSE_BYTES;
    request
        .widgets
        .into_iter()
        .filter_map(|key| {
            let mut view = plugins.widget(&key, &invocation)?;
            for widget in &mut view.content {
                if let Widget::Sparkline { samples, .. } = widget {
                    samples.retain(|value| value.is_finite());
                }
            }
            let update = WidgetUpdate { key, view };
            let bytes = serde_json::to_vec(&update).ok()?.len().saturating_add(1);
            bytes_left = bytes_left.checked_sub(bytes)?;
            Some(update)
        })
        .collect()
}

#[cfg(test)]
#[path = "plugin_host_tests.rs"]
mod tests;
