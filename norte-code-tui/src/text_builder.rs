use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::{
    components::{ComponentId, LifecyclePhase, palette},
    keys,
    ui_model::SendMode,
};

mod conversation;
mod data;

pub struct TextBuilder;

fn session_id_short(session: &str) -> String {
    session.chars().take(8).collect()
}

fn queue_mode_compact(mode: SendMode) -> &'static str {
    match mode {
        SendMode::Steer => "steer",
        SendMode::Queue => "queue",
        SendMode::Interrupt => "interrupt",
        SendMode::Reject => "reject",
    }
}

use crate::components::HeaderModal;

/// What a clickable header segment does when it is clicked.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HeaderAction {
    /// Opens the named floating menu.
    Modal(HeaderModal),
    /// Switches to the named view.
    OpenView(ComponentId),
}

/// The header as it is drawn, with the buttons in it.
pub(crate) struct HeaderLayout {
    pub(crate) line: Line<'static>,
    /// The clickable parts, with the columns they span.
    pub(crate) buttons: Vec<(usize, usize, HeaderAction)>,
}

struct HeaderBuilder {
    theme: crate::ui_model::ThemeMode,
    spans: Vec<Span<'static>>,
    buttons: Vec<(usize, usize, HeaderAction)>,
    column: usize,
}

impl HeaderBuilder {
    fn new(theme: crate::ui_model::ThemeMode) -> Self {
        Self {
            theme,
            spans: Vec::new(),
            buttons: Vec::new(),
            column: 0,
        }
    }

    fn push(&mut self, span: Span<'static>) {
        self.column += span.content.chars().count();
        self.spans.push(span);
    }

    fn muted(&mut self, text: String) {
        self.push(Span::styled(text, Style::default().fg(palette::muted())));
    }

    fn button(&mut self, text: String, action: HeaderAction) {
        let start = self.column;
        let style = palette::selected_style(self.theme).add_modifier(Modifier::BOLD);
        self.push(Span::styled(text, style));
        self.buttons.push((start, self.column, action));
    }

    fn finish(self) -> HeaderLayout {
        HeaderLayout {
            line: Line::from(self.spans),
            buttons: self.buttons,
        }
    }
}

/// A token count short enough to read.
fn human_count(value: u64) -> String {
    let (divisor, suffix) = if value >= 1_000_000 {
        (1_000_000, "M")
    } else if value >= 1_000 {
        (1_000, "k")
    } else {
        return value.to_string();
    };
    let whole = value / divisor;
    let tenth = (value % divisor) * 10 / divisor;
    format!("{whole}.{tenth}{suffix}")
}

impl TextBuilder {
    pub const SELECTED_MARKER: &'static str = "▸ ";

    /// The header as it is drawn, with the buttons in it and the columns
    /// each button spans.
    pub(crate) fn header_layout(model: &crate::ui_model::UiModel) -> HeaderLayout {
        let mut builder = HeaderBuilder::new(model.preferences.theme);
        builder.push(Span::styled(
            Self::application_title(),
            Style::default()
                .fg(palette::accent(builder.theme))
                .add_modifier(Modifier::BOLD),
        ));
        builder.push(Span::raw("  "));
        builder.muted(Self::application_status(model).to_owned());

        let session = model
            .active_session
            .as_ref()
            .map(|session| session_id_short(session.as_str()));
        let model_name = model
            .session_model
            .as_ref()
            .map(|model| format!("{}/{}", model.provider, model.model))
            .or_else(|| {
                model
                    .active_session
                    .as_ref()
                    .and_then(|session| {
                        model
                            .sessions
                            .iter()
                            .find(|summary| summary.session_id == session.as_str())
                    })
                    .and_then(|summary| {
                        Some(format!(
                            "{}/{}",
                            summary.provider.clone()?,
                            summary.model.clone()?.id
                        ))
                    })
            });

        for item in [session].into_iter().flatten() {
            builder.push(Span::raw(" · "));
            builder.muted(item);
        }
        if let Some(model) = model_name {
            builder.push(Span::raw(" · "));
            builder.button(model, HeaderAction::OpenView(ComponentId::Models));
        }

        builder.push(Span::raw(" · "));
        builder.button(
            format!(" queue {} ", queue_mode_compact(model.send_mode)),
            HeaderAction::Modal(HeaderModal::Queue),
        );
        builder.push(Span::raw(" · "));
        builder.button(
            format!(" reasoning {} ", model.reasoning.label()),
            HeaderAction::Modal(HeaderModal::Reasoning),
        );
        if let Some(stats) = model.session_stats {
            builder.push(Span::raw(" · "));
            builder.muted(Self::session_stats_compact(&stats));
        }
        builder.finish()
    }

    /// The session's usage, short enough to read at a glance.
    fn session_stats_compact(stats: &crate::ui_model::SessionStatsSummary) -> String {
        format!(
            "{} req · {} in · {} out · {} cached · ${:.4}",
            stats.requests,
            human_count(stats.input_tokens),
            human_count(stats.output_tokens),
            human_count(stats.cache_read_tokens),
            stats.cost_usd,
        )
    }

    #[must_use]
    pub fn tab_label(view: ComponentId) -> Line<'static> {
        Line::from(format!("{} {}", view.number(), Self::view_label(view)))
    }

    #[must_use]
    pub fn help_lines() -> Vec<Line<'static>> {
        vec![
            Line::from(format!("1-7/{}  switch views", crate::keys::COSTS_VIEW)),
            Line::from(format!(
                "{}  sessions · Enter expand/open · {} create",
                keys::SESSIONS_VIEW_CHAR,
                keys::NEW_SESSION_CHAR
            )),
            Line::from(format!("{}  move selection", keys::NAV_DOWN_CHAR)),
            Line::from(format!("{}  move selection", keys::NAV_UP_CHAR)),
            Line::from(format!(
                "{}  search providers/models; Enter expands/selects",
                keys::MODEL_SEARCH_CHAR
            )),
            Line::from(format!("{}  focus message composer", keys::COMPOSER_CHAR)),
            Line::from(format!("{}  send message", Self::submit_key_label())),
            Line::from(format!("{}  toggle edit diff mode", keys::DIFF_CHAR)),
            Line::from(format!("{}  stop the running turn", keys::CANCEL_TURN_CHAR)),
            Line::from("Ctrl+C  stop the running turn".to_owned()),
            Line::from(format!("{}  reasoning effort menu", keys::REASONING_CHAR)),
            Line::from(format!("{}  fold or unfold the turn", keys::FOLD_CHAR)),
            Line::from(format!(
                "{} {}  go to the previous or next turn",
                keys::PREV_TURN_CHAR,
                keys::NEXT_TURN_CHAR
            )),
            Line::from(format!("{}  toggle a turn's fold", keys::FOLD_CHAR)),
            Line::from("click a turn header to fold it, right-click it for a menu".to_owned()),
            Line::from("click the model name in the header to open the catalogue".to_owned()),
            Line::from("click the queue or reasoning pill in the header for a menu".to_owned()),
            Line::from(format!("{}  stop a background job", keys::STOP_JOB_CHAR)),
            Line::from(format!("{}  start a new session", keys::NEW_SESSION_CHAR)),
            Line::from(format!(
                "{}  import a session from a bundle",
                keys::IMPORT_CHAR
            )),
            Line::from(format!(
                "{}  cycle message queue mode",
                keys::QUEUE_MODE_CHAR
            )),
            Line::from(format!("{}  cycle focused panel", Self::focus_key_label())),
            Line::from(format!("{}  close help", Self::cancel_key_label())),
            Line::from(format!("{}  quit", keys::QUIT_CHAR)),
        ]
    }

    #[must_use]
    pub const fn help_title() -> &'static str {
        "keyboard help"
    }

    #[must_use]
    pub const fn application_title() -> &'static str {
        "NORTE"
    }

    #[must_use]
    pub fn application_status(model: &crate::ui_model::UiModel) -> &'static str {
        if model.server.is_some() || model.liveness.is_some() || model.readiness.is_some() {
            "connected · live messages"
        } else if model.host_errors.is_empty() {
            "connecting"
        } else {
            "connection errors"
        }
    }

    #[must_use]
    pub const fn panel_title_for(view: ComponentId) -> &'static str {
        match view {
            ComponentId::Session => "conversation",
            ComponentId::Plans => "plans",
            ComponentId::Telemetry => "requests",
            ComponentId::Plugins => "plugins",
            ComponentId::Models => "providers and models",
            ComponentId::Host => "capabilities",
            ComponentId::Sessions => "sessions",
            ComponentId::Jobs => "jobs",
            ComponentId::PlanReview => "plan review",
            ComponentId::Costs => "costs",
            ComponentId::Settings => "settings",
        }
    }

    #[must_use]
    pub const fn host_title() -> &'static str {
        "host"
    }

    #[must_use]
    pub const fn details_title() -> &'static str {
        "plan detail"
    }

    #[must_use]
    pub const fn submit_key_label() -> &'static str {
        "Enter"
    }

    #[must_use]
    pub const fn focus_key_label() -> &'static str {
        "Tab"
    }

    #[must_use]
    pub const fn view_label(view: ComponentId) -> &'static str {
        match view {
            ComponentId::Session => "Chat",
            ComponentId::Plans => "Plans",
            ComponentId::Telemetry => "Telemetry",
            ComponentId::Plugins => "Plugins",
            ComponentId::Models => "Models",
            ComponentId::Host => "Host",
            ComponentId::Sessions => "Sessions",
            ComponentId::Jobs => "Jobs",
            ComponentId::PlanReview => "Plan review",
            ComponentId::Costs => "Costs",
            ComponentId::Settings => "Settings",
        }
    }

    #[must_use]
    pub const fn phase_label(phase: LifecyclePhase) -> &'static str {
        match phase {
            LifecyclePhase::Dormant => "inactive",
            LifecyclePhase::Mounting => "starting",
            LifecyclePhase::Loading => "loading",
            LifecyclePhase::Ready => "ready",
            LifecyclePhase::Empty => "empty",
            LifecyclePhase::Updating => "updating",
            LifecyclePhase::Failed => "failed",
        }
    }

    #[must_use]
    pub fn panel_title(view: ComponentId, phase: LifecyclePhase) -> Line<'static> {
        Line::from(vec![
            Span::raw(Self::panel_title_for(view)),
            Span::raw(" · "),
            Span::styled(Self::phase_label(phase), Self::phase_style(phase)),
        ])
    }

    #[must_use]
    pub fn phase_style(phase: LifecyclePhase) -> Style {
        match phase {
            LifecyclePhase::Dormant | LifecyclePhase::Empty => {
                Style::default().fg(palette::muted())
            }
            LifecyclePhase::Mounting | LifecyclePhase::Loading | LifecyclePhase::Updating => {
                Style::default().fg(palette::warning())
            }
            LifecyclePhase::Ready => Style::default().fg(palette::success()),
            LifecyclePhase::Failed => Style::default()
                .fg(palette::danger())
                .add_modifier(Modifier::BOLD),
        }
    }

    #[must_use]
    pub const fn empty_message(view: ComponentId) -> &'static str {
        match view {
            ComponentId::Session => "No active session",
            ComponentId::Plans => "No plans loaded",
            ComponentId::Telemetry => "No telemetry loaded",
            ComponentId::Plugins => "No plugins loaded",
            ComponentId::Models => "No models loaded",
            ComponentId::Host => "No host capabilities loaded",
            ComponentId::Sessions => "No sessions loaded",
            ComponentId::Jobs => "No jobs in this session",
            ComponentId::PlanReview => "No plan review is active",
            ComponentId::Costs => "No cost reports loaded",
            ComponentId::Settings => "No settings",
        }
    }

    #[must_use]
    pub const fn transcript_empty() -> &'static str {
        "No messages yet"
    }

    #[must_use]
    pub const fn no_model_matches() -> &'static str {
        "No providers or models match this filter"
    }

    #[must_use]
    pub fn composer_mode_placeholder(key: char) -> String {
        format!("Press {key} to compose")
    }

    #[must_use]
    pub const fn budget_action_description(action: norte_code_wire::BudgetAction) -> &'static str {
        match action {
            norte_code_wire::BudgetAction::Warn => "warn and continue",
            norte_code_wire::BudgetAction::Ask => "ask before one continuation",
            norte_code_wire::BudgetAction::Stop => "stop the turn",
        }
    }

    #[must_use]
    pub const fn composer_focused_placeholder() -> &'static str {
        "Type a message, Enter sends, Esc leaves the composer"
    }

    #[must_use]
    pub const fn composer_busy_placeholder() -> &'static str {
        "Wait for the current response to finish"
    }

    #[must_use]
    pub const fn child_session_read_only() -> &'static str {
        "Subagent session · view only"
    }

    #[must_use]
    pub const fn plan_review_title() -> &'static str {
        "review proposed plan"
    }

    #[must_use]
    pub const fn plan_review_feedback_title() -> &'static str {
        "feedback"
    }

    #[must_use]
    pub const fn plan_review_feedback_hint() -> &'static str {
        "Press f to write feedback"
    }

    #[must_use]
    pub const fn question_title() -> &'static str {
        "question"
    }

    #[must_use]
    pub const fn question_answer_hint() -> &'static str {
        "Press f to enter an answer"
    }

    #[must_use]
    pub const fn plan_review_controls(editing_feedback: bool) -> &'static str {
        if editing_feedback {
            "Enter send feedback · Esc cancel"
        } else {
            "Enter/a approve · d reject · f feedback"
        }
    }

    #[must_use]
    pub const fn plan_selection_hint() -> &'static str {
        "Select a plan to inspect it"
    }

    #[must_use]
    pub const fn telemetry_unavailable() -> &'static str {
        "Summary unavailable"
    }

    #[must_use]
    pub const fn host_unavailable() -> &'static str {
        "Server information has not been loaded"
    }

    #[must_use]
    pub const fn cancel_key_label() -> &'static str {
        keys::CANCEL_LABEL
    }

    #[must_use]
    pub fn footer_ticker(view: ComponentId, offset: usize, width: usize) -> String {
        if width == 0 {
            return String::new();
        }
        let source = Self::footer_help(view);
        if source.chars().count() <= width {
            return source;
        }
        let separator = "   ·   ";
        let ticker = format!("{source}{separator}");
        let ticker_width = ticker.chars().count();
        let offset = offset % ticker_width;
        let repeated = ticker.repeat(3);
        repeated.chars().skip(offset).take(width).collect()
    }

    #[must_use]
    pub fn footer_help(view: ComponentId) -> String {
        let controls = match view {
            ComponentId::Session => format!(
                "{}/{} navigate · {} compose · Esc/Tab focus · {} send · {} diff · {} queue · {} budget · {} export · {} compact · {} cancel · {} reasoning · {} new · {} import · {} logs",
                keys::NAV_DOWN_CHAR,
                keys::NAV_UP_CHAR,
                keys::COMPOSER_CHAR,
                Self::submit_key_label(),
                keys::DIFF_CHAR,
                keys::QUEUE_MODE_CHAR,
                keys::BUDGET_CHAR,
                keys::EXPORT_CHAR,
                keys::COMPACT_CHAR,
                keys::CANCEL_TURN_CHAR,
                keys::REASONING_CHAR,
                keys::NEW_SESSION_CHAR,
                keys::IMPORT_CHAR,
                'o'
            ),
            ComponentId::Jobs => format!(
                "{}/{} navigate · {} stop · {} refresh",
                keys::NAV_DOWN_CHAR,
                keys::NAV_UP_CHAR,
                keys::STOP_JOB_CHAR,
                'r'
            ),
            ComponentId::Plans => format!(
                "{}/{} navigate · Enter details",
                keys::NAV_DOWN_CHAR,
                keys::NAV_UP_CHAR
            ),
            ComponentId::Telemetry => format!(
                "{}/{} navigate · [/] page · r refresh · t scope",
                keys::NAV_DOWN_CHAR,
                keys::NAV_UP_CHAR
            ),
            ComponentId::Plugins => format!(
                "{}/{} navigate · r refresh · i install · u update · U all · e toggle · x remove",
                keys::NAV_DOWN_CHAR,
                keys::NAV_UP_CHAR
            ),
            ComponentId::Models => format!(
                "{} search · {}/{} navigate · Enter select",
                keys::MODEL_SEARCH_CHAR,
                keys::NAV_DOWN_CHAR,
                keys::NAV_UP_CHAR
            ),
            ComponentId::Host => format!(
                "Tab sections · {}/{} navigate · r refresh · Enter MCP · a/d/f approvals",
                keys::NAV_DOWN_CHAR,
                keys::NAV_UP_CHAR
            ),
            ComponentId::Sessions => format!(
                "Enter expand/open · {} fold · r rename · d delete · f fork · {}/{} navigate · {} create",
                keys::CANCEL,
                keys::NAV_DOWN_CHAR,
                keys::NAV_UP_CHAR,
                keys::NEW_SESSION_CHAR
            ),
            ComponentId::Costs => "r refresh".to_owned(),
            ComponentId::Settings => "j/k select · Enter toggle · r test connection".to_owned(),
            ComponentId::PlanReview => "Enter respond".to_owned(),
        };
        format!(
            "1-7/{} views · v models · g settings · s sessions · {controls} · {} quit · {} help",
            keys::COSTS_VIEW,
            keys::QUIT_CHAR,
            keys::HELP_CHAR
        )
    }
}

#[cfg(test)]
#[path = "text_builder_tests.rs"]
mod tests;
