//! Client-independent declarations and content for plugin widgets.

use serde::{Deserialize, Serialize};

use crate::SessionKey;

/// The caller's selection, including a model picked before creating a session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginSelection {
    /// The attached session, if any.
    pub session: Option<SessionKey>,
    /// The provider serving the selected model.
    pub provider: Option<String>,
    /// The selected model id.
    pub model: Option<String>,
}

/// A widget identifier within one plugin.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WidgetId(String);

impl WidgetId {
    /// Constructs a widget identifier from its stable name.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The name supplied by the declaring plugin.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A widget owned by one loaded plugin.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WidgetKey {
    /// The declared plugin name.
    pub plugin: String,
    /// The widget's id in that plugin.
    pub widget: WidgetId,
}

/// A widget that a client can choose to request and display.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WidgetRegistration {
    /// The widget's address.
    pub key: WidgetKey,
    /// A human-readable description for the client.
    pub title: String,
}

/// Reads selected widgets for one client.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginWidgetRequest {
    /// Effective selection for this request.
    pub selection: PluginSelection,
    /// Widgets chosen by the client.
    pub widgets: Vec<WidgetKey>,
    /// Wait for an invalidation after this revision, when supplied.
    pub after: Option<u64>,
}

/// Content that can be rendered by any client.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WidgetView {
    /// Optional heading for a group of widgets.
    pub title: Option<String>,
    /// Content in display order.
    pub content: Vec<Widget>,
}

/// A semantic severity for a badge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BadgeTone {
    /// Ordinary status.
    #[default]
    Neutral,
    /// Positive status.
    Success,
    /// Attention required.
    Warning,
    /// Failed or critical status.
    Error,
}

/// One named value in a property list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WidgetField {
    /// The name of the field.
    pub label: String,
    /// Its display value.
    pub value: String,
}

/// Content a client can translate into its own controls and components.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Widget {
    /// Plain text.
    Text {
        /// The text to display.
        text: String,
    },
    /// A percentage meter.
    Progress {
        /// The name of the meter.
        label: String,
        /// Percentage from 0 to 100.
        percent: u8,
        /// Optional reset time, as Unix seconds.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resets_at: Option<i64>,
    },
    /// A list of items.
    List {
        /// The items in display order.
        items: Vec<String>,
    },
    /// A set of named values.
    KeyValue {
        /// The fields in display order.
        fields: Vec<WidgetField>,
    },
    /// Tabular records.
    Table {
        /// Column titles in display order.
        columns: Vec<String>,
        /// Records whose values correspond to the columns.
        records: Vec<Vec<String>>,
    },
    /// A status label.
    Badge {
        /// The badge text.
        text: String,
        /// Semantic tone.
        #[serde(default)]
        tone: BadgeTone,
    },
    /// A compact plot of a time series.
    Sparkline {
        /// Optional label.
        label: Option<String>,
        /// Samples in time order, oldest first.
        samples: Vec<f64>,
    },
    /// A separator between groups.
    Divider,
}

/// The current content of one requested widget.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WidgetUpdate {
    /// The widget identity used for placement.
    pub key: WidgetKey,
    /// The visible content.
    pub view: WidgetView,
}

/// A snapshot of the widgets visible for this client's selection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginWidgetSnapshot {
    /// Revision read before evaluating the plugins.
    pub revision: u64,
    /// Visible widgets in the caller's requested order.
    pub updates: Vec<WidgetUpdate>,
}
