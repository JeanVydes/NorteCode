use super::*;

use norte_code_plugin_host::{Plugin, WidgetDefinition};
use norte_code_wire::{
    SessionKey,
    plugin_widgets::{Widget, WidgetId, WidgetView},
};

const OWNER: &str = "example";
const USAGE: &str = "usage";
const OTHER: &str = "other";

struct PerSession;

impl Plugin for PerSession {
    fn name(&self) -> &'static str {
        OWNER
    }

    fn widgets(&self) -> Vec<WidgetDefinition> {
        [USAGE, OTHER]
            .into_iter()
            .map(|id| WidgetDefinition {
                id: WidgetId::new(id),
                title: id.to_owned(),
            })
            .collect()
    }

    fn widget(&self, id: &WidgetId, invocation: &PluginInvocation) -> Option<WidgetView> {
        let selection = &invocation.selection;
        if id.as_str() != USAGE || selection.provider.as_deref() != Some("codex") {
            return None;
        }
        Some(WidgetView {
            title: None,
            content: vec![Widget::Text {
                text: selection.session.as_ref()?.as_str().to_owned(),
            }],
        })
    }
}

fn request(session: &str, provider: &str, after: Option<u64>) -> PluginWidgetRequest {
    PluginWidgetRequest {
        selection: PluginSelection {
            session: Some(SessionKey::new(session)),
            provider: Some(provider.to_owned()),
            model: Some("example".to_owned()),
        },
        widgets: vec![WidgetKey {
            plugin: OWNER.to_owned(),
            widget: WidgetId::new(USAGE),
        }],
        after,
    }
}

#[tokio::test]
async fn updating_one_widget_preserves_every_clients_selection() {
    let mut plugins = Plugins::new();
    plugins.add(Box::new(PerSession));
    let host = PluginHostAdapter::new();
    let widgets = plugins.widget_registrations();
    host.install_plugins(Arc::new(plugins), &widgets);
    let first = host.widgets(request("first", "codex", None), None).await;
    let second = host.widgets(request("second", "openai", None), None).await;
    assert_eq!(second.updates, []);

    let waiting = host.widgets(request("first", "codex", Some(first.revision)), None);
    tokio::pin!(waiting);
    host.context(OWNER)
        .request_widget_update(&WidgetId::new(OTHER));
    host.context("different-plugin")
        .request_widget_update(&WidgetId::new(USAGE));
    assert!(
        tokio::time::timeout(Duration::from_millis(15), &mut waiting)
            .await
            .is_err()
    );

    host.context(OWNER)
        .request_widget_update(&WidgetId::new(USAGE));
    let refreshed = tokio::time::timeout(Duration::from_secs(1), waiting)
        .await
        .unwrap();
    assert_ne!(first.revision, refreshed.revision);
    assert_eq!(first.updates, refreshed.updates);
    assert_eq!(
        refreshed.updates[0].view.content,
        [Widget::Text {
            text: "first".to_owned()
        }]
    );
}
