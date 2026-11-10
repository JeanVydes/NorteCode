use crate::api::ApiClient;
use crate::domain::Capability;
use crate::state::AppContext;
use dioxus::prelude::*;
use norte_code_wire::ChildState as ChildSessionState;

/// Which pane the session header shows under the conversation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SessionTab {
    #[default]
    Transcript,
    Subagents,
    Jobs,
    Plans,
    Telemetry,
}

impl SessionTab {
    pub const ALL: [Self; 5] = [
        Self::Transcript,
        Self::Subagents,
        Self::Jobs,
        Self::Plans,
        Self::Telemetry,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Transcript => "Transcript",
            Self::Subagents => "Subagents",
            Self::Jobs => "Jobs",
            Self::Plans => "Plans",
            Self::Telemetry => "Telemetry",
        }
    }
}

/// The session header: the title, what the turn is doing, and the panes that
/// belong to this session. Model, subagents, jobs and telemetry are all
/// session-scoped, so they live here rather than in global navigation.
#[component]
pub fn SessionHeader(
    session_title: String,
    tab: SessionTab,
    on_tab: EventHandler<SessionTab>,
    thinking: Option<String>,
    on_compact: EventHandler<()>,
) -> Element {
    let context = use_context::<AppContext>();
    let compacting = context.is_compacting();
    rsx! {
        header { class: "session-header",
            div { class: "session-header-top",
                h1 { class: "session-header-title", "{session_title}" }
                if let Some(label) = thinking {
                    span { class: "thinking",
                        span { class: "thinking-dot" }
                        "{label}"
                    }
                }
                details { class: "menu",
                    summary { class: "link-button", title: "Session settings", "Settings" }
                    div { class: "menu-items",
                        SessionSettings {}
                    }
                }
                button {
                    class: "link-button",
                    disabled: compacting,
                    title: "Summarise the history now and keep the most recent turns",
                    onclick: move |_| on_compact.call(()),
                    if compacting { "Compacting…" } else { "Compact" }
                }
            }
            nav { class: "session-tabs",
                for entry in SessionTab::ALL {
                    button {
                        class: if entry == tab { "session-tab session-tab-active" } else { "session-tab" },
                        disabled: entry == SessionTab::Subagents
                            && !delegation_supported(&context),
                        onclick: move |_| on_tab.call(entry),
                        "{entry.label()}"
                    }
                }
            }
        }
    }
}

/// The knobs that belong to one session: how it reasons, what it may spend,
/// and how a message typed during a turn is handled.
#[component]
fn SessionSettings() -> Element {
    let context = use_context::<AppContext>();
    let session_id = context
        .selected_session
        .read()
        .as_ref()
        .map(|session| session.session_id.clone())
        .unwrap_or_default();
    let mut budget_draft = use_signal(String::new);
    let export_url = use_signal(String::new);

    rsx! {
        div { class: "menu-section",
            span { class: "menu-label", "Export" }
            if export_url.read().is_empty() {
                button {
                    class: "popover-item",
                    onclick: {
                        let session_id = session_id.clone();
                        move |_| export_session(&context, session_id.clone(), export_url)
                    },
                    "Prepare export"
                }
            } else {
                a {
                    class: "popover-item",
                    href: "{export_url}",
                    download: "session.raep.zip",
                    "Download bundle"
                }
            }
        }
        div { class: "menu-section",
            span { class: "menu-label", "Session budget (USD)" }
            input {
                class: "text-input",
                r#type: "number",
                step: "0.5",
                min: "0",
                placeholder: "No limit",
                value: "{budget_draft}",
                oninput: move |event| budget_draft.set(event.value()),
                onkeydown: {
                    let session_id = session_id.clone();
                    move |event| {
                        if event.key() != Key::Enter {
                            return;
                        }
                        let limit = budget_draft.read().trim().parse::<f64>().unwrap_or(0.0);
                        if limit > 0.0 {
                            set_budget(&context, session_id.clone(), limit);
                        }
                    }
                },
            }
        }
    }
}

/// Packages the session on the host and keeps the download link.
fn export_session(context: &AppContext, session_id: String, mut export_url: Signal<String>) {
    let context = *context;
    spawn(async move {
        let Some(client) = client(&context) else {
            return;
        };
        match client.export_session(&session_id).await {
            Ok(export_id) => match client.export_url(&export_id) {
                Ok(url) => export_url.set(url),
                Err(error) => context.set_page_error(Some(error.to_string())),
            },
            Err(error) => {
                context.set_page_error(Some(format!("Could not export the session: {error}")));
            }
        }
    });
}

fn set_budget(context: &AppContext, session_id: String, limit: f64) {
    let context = *context;
    spawn(async move {
        let Some(client) = client(&context) else {
            return;
        };
        let budget = norte_code_wire::Budget {
            unit: norte_code_wire::BudgetUnit::Usd,
            limit,
            action: norte_code_wire::BudgetAction::Warn,
        };
        if let Err(error) = client.set_budget(&session_id, budget).await {
            context.set_page_error(Some(format!("Could not set the budget: {error}")));
        }
    });
}

fn delegation_supported(context: &AppContext) -> bool {
    context
        .server_info
        .read()
        .as_ref()
        .is_some_and(|info| info.capabilities.contains(&Capability::DelegationBasic))
}

fn client(context: &AppContext) -> Option<ApiClient> {
    let profile = context.active_profile()?;
    ApiClient::new(&profile.base_url, &profile.bearer_token).ok()
}

/// Child sessions of the open session, loaded on demand.
#[component]
pub fn SessionSubagents() -> Element {
    let context = use_context::<AppContext>();
    let selected = context.selected_session.read().clone();
    let Some(session) = selected else {
        return rsx! { crate::components::EmptyState { title: "No session", message: "" } };
    };
    let session_id = session.session_id.clone();
    // The host is the source of truth; the stream is the live view. Ask once
    // when the pane opens, then let `ChildSessionStarted/Updated` keep it
    // current — a spawn shows up the moment it happens, not on a reload.
    use_effect(use_reactive((&session_id,), {
        move |(session_id,)| {
            spawn(async move {
                if let Some(client) = client(&context)
                    && let Ok(list) = client.child_sessions(&session_id).await
                {
                    context.set_children(list);
                }
            });
        }
    }));

    let children = context.children.read().clone();
    if children.is_empty() {
        return rsx! {
            crate::components::EmptyState {
                title: "No subagents",
                message: "This session has not spawned a child session."
            }
        };
    }
    rsx! {
        div { class: "subagent-list",
            for child in children {
                SubagentCard { key: "{child.session_id}", child: child.clone() }
            }
        }
    }
}

/// One child session: who it is, what it was asked, how long it has run, and
/// the way into its own transcript.
#[component]
fn SubagentCard(child: norte_code_wire::ChildSessionSummary) -> Element {
    let context = use_context::<AppContext>();
    let elapsed = child
        .started_at
        .and_then(|started| {
            crate::api::now_millis()
                .ok()
                .map(|now| (now - started).max(0))
        })
        .map(|ms| {
            if ms >= 60_000 {
                format!("{}m {}s", ms / 60_000, (ms % 60_000) / 1_000)
            } else {
                format!("{}s", ms / 1_000)
            }
        });
    let session_id = child.session_id.clone();
    rsx! {
        article { class: "subagent",
            div { class: "subagent-head",
                span { class: child_state_class(child.state), "{child.state.label()}" }
                if let Some(agent) = &child.agent {
                    span { class: "subagent-agent", "{agent}" }
                }
                if let Some(elapsed) = elapsed {
                    span { class: "session-row-meta", "{elapsed}" }
                }
            }
            if let Some(task) = &child.task {
                p { class: "subagent-task", "{task}" }
            } else {
                p { class: "subagent-task helper-text",
                    "{child.title.clone().unwrap_or_else(|| \"No task recorded\".to_owned())}"
                }
            }
            div { class: "row-actions",
                button {
                    class: "link-button",
                    title: "Open this subagent's own transcript",
                    onclick: move |_| {
                        context.select_session(
                            norte_code_wire::SessionSummary {
                                session_id: session_id.clone(),
                                title: child.title.clone(),
                                ..Default::default()
                            },
                            crate::domain::Page::Chat,
                        );
                    },
                    "Open session"
                }
                span { class: "mono session-row-meta", "{child.session_id}" }
            }
        }
    }
}

fn child_state_class(state: ChildSessionState) -> &'static str {
    if state.is_working() {
        "status status-active"
    } else if state == ChildSessionState::Failed {
        "status status-error"
    } else {
        "status status-muted"
    }
}

/// The open session's provider telemetry: requests, tokens, latency and cost
/// for this session only.
#[component]
pub fn SessionTelemetry() -> Element {
    let context = use_context::<AppContext>();
    let report = use_signal(|| None::<norte_code_wire::TelemetryReport>);
    let selected = context.selected_session.read().clone();
    let Some(session) = selected else {
        return rsx! { crate::components::EmptyState { title: "No session", message: "" } };
    };
    let session_id = session.session_id.clone();
    use_effect({
        let session_id = session_id.clone();
        move || {
            let session_id = session_id.clone();
            spawn(async move {
                if let Some(client) = client(&context)
                    && let Ok(loaded) = client.telemetry(Some(&session_id)).await
                {
                    let mut report = report;
                    report.set(Some(loaded));
                }
            });
        }
    });
    let report = report.read().clone();
    let Some(report) = report else {
        return rsx! { p { class: "helper-text", "Loading…" } };
    };
    let summary = &report.summary;
    rsx! {
        div { class: "session-metrics",
            for (value, label) in [
                (summary.requests.to_string(), "requests"),
                (summary.input_tokens.to_string(), "input tokens"),
                (summary.output_tokens.to_string(), "output tokens"),
                (format!("{} ms", summary.duration_ms), "total time"),
                (summary.errors.to_string(), "errors"),
            ] {
                div { class: "session-metric",
                    span { class: "session-metric-value", "{value}" }
                    span { class: "session-metric-label", "{label}" }
                }
            }
        }
        if report.records.is_empty() {
            crate::components::EmptyState {
                title: "No requests yet",
                message: "Send a message and its provider metrics appear here."
            }
        } else {
            div { class: "session-list",
                for record in report.records.iter().rev().take(20) {
                    div { class: "session-row", key: "{record.id}",
                        span { class: "session-row-title", "{record.provider} / {record.model}" }
                        span { class: "session-row-meta",
                            "{record.duration_ms} ms · \
                             {record.input_tokens.unwrap_or(0)} in · \
                             {record.output_tokens.unwrap_or(0)} out"
                        }
                    }
                }
            }
        }
    }
}
