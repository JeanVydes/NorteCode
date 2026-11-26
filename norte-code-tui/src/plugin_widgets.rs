//! Follows invalidations for the widgets selected by this TUI.

use std::sync::Arc;
use std::time::Duration;

use norte_code_client::api::RestClient;
use norte_code_wire::plugin_widgets::{PluginSelection, PluginWidgetRequest, WidgetKey};
use tokio::sync::{mpsc, watch};

use crate::client::{ClientEvent, UiCommand};
use crate::ui_model::UiModel;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Scope {
    pub(crate) selection: PluginSelection,
    pub(crate) widgets: Vec<WidgetKey>,
}

pub(crate) async fn follow(
    api: Arc<RestClient>,
    mut selection: watch::Receiver<Option<Scope>>,
    events: mpsc::Sender<ClientEvent>,
) {
    let mut after = None;
    let mut previous = None;
    loop {
        let current = selection.borrow_and_update().clone();
        if current != previous {
            after = None;
            previous.clone_from(&current);
        }
        let Some(current) = current else {
            if selection.changed().await.is_err() {
                return;
            }
            after = None;
            continue;
        };
        let request = PluginWidgetRequest {
            selection: current.selection.clone(),
            widgets: current.widgets.clone(),
            after,
        };
        let result = tokio::select! {
            biased;
            change = selection.changed() => {
                if change.is_err() { return; }
                after = None;
                continue;
            }
            result = api.plugin_widgets(&request) => result,
            () = events.closed() => return,
        };
        if let Ok(snapshot) = result {
            after = Some(snapshot.revision);
            if events
                .send(ClientEvent::PluginWidgets(current, snapshot.updates))
                .await
                .is_err()
            {
                return;
            }
        } else {
            if after.take().is_some()
                && events
                    .send(ClientEvent::PluginWidgets(current, Vec::new()))
                    .await
                    .is_err()
            {
                return;
            }
            tokio::select! {
                result = selection.changed() => if result.is_err() { return; },
                () = tokio::time::sleep(Duration::from_secs(5)) => {},
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct Subscription {
    sent: Option<(mpsc::Sender<UiCommand>, Option<Scope>)>,
}

impl Subscription {
    pub(crate) fn sync(&mut self, commands: Option<&mpsc::Sender<UiCommand>>, model: &UiModel) {
        let Some(commands) = commands else {
            self.sent = None;
            return;
        };
        let selection = model.plugin_widget_scope();
        if self.sent.as_ref().is_some_and(|(sender, previous)| {
            sender.same_channel(commands) && *previous == selection
        }) {
            return;
        }
        if commands
            .try_send(UiCommand::WatchPluginWidgets(selection.clone()))
            .is_ok()
        {
            self.sent = Some((commands.clone(), selection));
        }
    }
}
