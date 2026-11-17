use norte_code_wire::ServerEventKind;

use super::App;
use crate::{
    components::{ComponentAction, ComponentId, McpState, Preference, ViewAction},
    ui_model::{
        SendMode, SessionAttachmentState, ThemeMode, ThinkingVisibility, TranscriptItem, TurnState,
    },
};

/// The capability a host advertises when it accepts uploads.
const UPLOADS_CAPABILITY: &str = "uploads";

/// The text a run of deltas adds to, leaving reasoning out.
fn aggregated_text(items: &[TranscriptItem]) -> String {
    items
        .iter()
        .filter_map(|item| match item {
            TranscriptItem::ServerEvent(event) => match &event.kind {
                ServerEventKind::Delta {
                    delta:
                        norte_code_wire::Delta::Text { text, .. }
                        | norte_code_wire::Delta::Refusal { text, .. },
                    ..
                } => Some(text.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// The reasoning a run of deltas adds to.
fn reasoning_text(items: &[TranscriptItem]) -> String {
    items
        .iter()
        .filter_map(|item| match item {
            TranscriptItem::ServerEvent(event) => match &event.kind {
                ServerEventKind::Delta {
                    delta: norte_code_wire::Delta::Reasoning { text, .. },
                    ..
                } => Some(text.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// Who spoke, the way a history page would record a replayed message.
fn replayed_speaker(role: norte_code_wire::MessageRole) -> norte_code_wire::HistorySpeaker {
    match role {
        norte_code_wire::MessageRole::User => norte_code_wire::HistorySpeaker::User,
        norte_code_wire::MessageRole::Assistant => norte_code_wire::HistorySpeaker::Assistant,
        norte_code_wire::MessageRole::Tool => norte_code_wire::HistorySpeaker::Tool,
        norte_code_wire::MessageRole::System
        | norte_code_wire::MessageRole::Developer
        | norte_code_wire::MessageRole::Unknown => norte_code_wire::HistorySpeaker::SystemEvent,
    }
}

/// The kind the host gives a replayed message in a history page.
fn replayed_kind(
    speaker: norte_code_wire::HistorySpeaker,
    kind: norte_code_wire::ReplayedKind,
) -> &'static str {
    match kind {
        norte_code_wire::ReplayedKind::Text => match speaker {
            norte_code_wire::HistorySpeaker::User => "user",
            norte_code_wire::HistorySpeaker::Assistant => "assistant",
            norte_code_wire::HistorySpeaker::Tool => "tool",
            norte_code_wire::HistorySpeaker::SystemEvent => "system_event",
        },
        norte_code_wire::ReplayedKind::Reasoning => "reasoning",
        norte_code_wire::ReplayedKind::ToolCall => "tool_call",
        norte_code_wire::ReplayedKind::ToolResult => "tool_result",
        norte_code_wire::ReplayedKind::Refusal => "refusal",
    }
}

/// Whether a history page covers one live item exactly.
fn history_covers(history: &[norte_code_wire::HistoryItem], item: &TranscriptItem) -> bool {
    match item {
        TranscriptItem::UserMessage { text, turn_id, .. } => {
            let by_id = turn_id.as_deref().is_some_and(|id| {
                history.iter().any(|entry| {
                    entry.role == norte_code_wire::HistorySpeaker::User
                        && entry.turn_id.as_deref() == Some(id)
                })
            });
            by_id
                || history.iter().any(|entry| {
                    entry.role == norte_code_wire::HistorySpeaker::User && entry.content == *text
                })
        }
        TranscriptItem::ServerEvent(event) => match &event.kind {
            ServerEventKind::MessageReplayed {
                role, kind, text, ..
            } => {
                let speaker = replayed_speaker(*role);
                let kind = replayed_kind(speaker, *kind);
                history.iter().any(|entry| {
                    entry.role == speaker && entry.kind == kind && entry.content == *text
                })
            }
            _ => false,
        },
        _ => false,
    }
}

/// The pieces of a live transcript that a history has replaced.
pub(super) fn drop_replaced_deltas(
    history: &[norte_code_wire::HistoryItem],
    current: Vec<TranscriptItem>,
) -> Vec<TranscriptItem> {
    let aggregate = aggregated_text(&current);
    let reasoning = reasoning_text(&current);
    let text_replayed = !aggregate.is_empty()
        && history.iter().any(|item| {
            item.role == norte_code_wire::HistorySpeaker::Assistant
                && item.kind != "reasoning"
                && item.content.contains(&aggregate)
        });
    let reasoning_replayed = !reasoning.is_empty()
        && history
            .iter()
            .any(|item| item.kind == "reasoning" && item.content.contains(&reasoning));
    current
        .into_iter()
        .filter(|item| match item {
            TranscriptItem::Status(_) | TranscriptItem::History(_) => true,
            TranscriptItem::ServerEvent(event) => match &event.kind {
                ServerEventKind::Delta { delta, .. } => match delta {
                    norte_code_wire::Delta::Text { .. }
                    | norte_code_wire::Delta::Refusal { .. } => !text_replayed,
                    norte_code_wire::Delta::Reasoning { .. } => !reasoning_replayed,
                    _ => !text_replayed && !reasoning_replayed,
                },
                ServerEventKind::ToolStarted { .. }
                | ServerEventKind::ToolOutput { .. }
                | ServerEventKind::ToolFinished { .. } => !text_replayed,
                _ => !history_covers(history, item),
            },
            _ => !history_covers(history, item),
        })
        .collect()
}

pub(super) fn transcript_with_history(
    history: Vec<norte_code_wire::HistoryItem>,
    current: Vec<TranscriptItem>,
    preserve_live: bool,
) -> Vec<TranscriptItem> {
    if !preserve_live {
        let mut seen = std::collections::HashSet::with_capacity(history.len());
        let mut transcript: Vec<TranscriptItem> = history
            .into_iter()
            .filter(|item| seen.insert(item.id.clone()))
            .map(TranscriptItem::History)
            .collect();
        // The compaction notice and any error are not part of the history
        // page, so they are the only things that survive a replacement.
        transcript.extend(
            current.into_iter().filter(|item| {
                matches!(item, TranscriptItem::Status(_) | TranscriptItem::Error(_))
            }),
        );
        return transcript;
    }

    // De-duplicate the history page by stable id, keeping first occurrence.
    let mut seen = std::collections::HashSet::with_capacity(history.len());
    let history: Vec<_> = history
        .into_iter()
        .filter(|item| seen.insert(item.id.clone()))
        .collect();
    let history_ids: std::collections::HashSet<String> =
        history.iter().map(|item| item.id.clone()).collect();
    let live = drop_replaced_deltas(&history, current);

    // The history page is the committed record and forms the base; live
    // items the page does not cover follow it, keeping their order.
    let mut transcript = Vec::with_capacity(history.len() + live.len());
    transcript.extend(history.into_iter().map(TranscriptItem::History));
    for item in live {
        if let TranscriptItem::History(item) = &item
            && history_ids.contains(item.id.as_str())
        {
            continue;
        }
        transcript.push(item);
    }
    transcript
}

impl App {
    pub(super) fn handle_component_action(
        &mut self,
        action: ComponentAction,
    ) -> Result<(), super::AppError> {
        match action {
            ComponentAction::OpenSession(session) => {
                if self.model.active_session.as_ref() != Some(&session)
                    || self.model.active_child.is_some()
                    || self.model.attachment_state != SessionAttachmentState::Attached
                {
                    self.open_session(session);
                }
                self.activate_view(crate::components::ComponentId::Session)?;
            }
            ComponentAction::OpenChildSession(session) => {
                self.open_child_session(session);
                self.activate_view(crate::components::ComponentId::Session)?;
            }
            ComponentAction::CreateSession => {
                self.client.create_session();
            }
            ComponentAction::RenameSession { session, title } => {
                self.client
                    .rename_session(session, title, self.session_generation);
            }
            ComponentAction::DeleteSession(session) => {
                self.client.delete_session(session, self.session_generation);
            }
            ComponentAction::ForkSession(session) => {
                self.client.fork_session(session, self.session_generation);
            }
            ComponentAction::SendMessage { text, chips } => self.send_message(text, &chips),
            ComponentAction::AttachFile(path) => self.attach_file(path),
            ComponentAction::ImportSession(path) => self.import_session(path),
            ComponentAction::ScrollTranscript(lines) => {
                if let Some(index) = self.view_index(ComponentId::Session)
                    && let Some(component) = self.views.get_mut(index)
                {
                    component.scroll_transcript(lines);
                }
            }
            ComponentAction::SetQueueMode(mode) => {
                self.model.send_mode = mode;
            }
            ComponentAction::SetModel { provider, model } => {
                if let Some(session) = self.model.active_session.clone()
                    && self.model.active_child.is_none()
                {
                    self.model.notice = Some(format!(
                        "Model change requested: {provider}/{model}. It applies at the next turn boundary."
                    ));
                    self.client
                        .set_model(session, self.session_generation, provider, model);
                }
            }
            ComponentAction::SetBudget { limit, action } => self.set_budget(limit, action),
            ComponentAction::ApproveBudget => self.approve_budget(),
            ComponentAction::DeclineBudget => self.decline_budget(),
            ComponentAction::CancelTurn => self.cancel_turn(),
            ComponentAction::OpenHeaderModal(kind) => {
                self.header_select = Some(super::HeaderSelectState::new(
                    kind,
                    &self.model,
                    self.catalog_capabilities(),
                ));
            }
            ComponentAction::SetReasoning(level) => self.set_reasoning(level),
            ComponentAction::Rewind { turn_id } => self.rewind(turn_id),
            ComponentAction::PinTurn { turn_id, pinned } => self.pin_turn(turn_id, pinned),
            ComponentAction::StopJob { job_id } => self.stop_job(job_id),
            ComponentAction::SetDiffStyle(diff_style) => {
                self.model.cli.diff_style = diff_style;
                self.persist_preferences();
            }
            ComponentAction::AnswerQuestion {
                session,
                question,
                answer,
            } => self.answer_question(session, question, answer),
            ComponentAction::View(action) => self.handle_view_action(action),
        }
        Ok(())
    }

    fn handle_view_action(&mut self, action: ViewAction) {
        match action {
            ViewAction::RefreshHost | ViewAction::TestConnection => {
                self.client.load_host(self.session_generation);
            }
            ViewAction::RefreshSkills => self.client.load_skills(self.session_generation, true),
            ViewAction::RefreshMcp => self.client.load_mcp(self.session_generation, true),
            ViewAction::SetMcp { name, state } => self.client.set_mcp_active(
                name,
                matches!(state, McpState::Active),
                self.session_generation,
            ),
            ViewAction::SetToolRule { name, action } => {
                self.client
                    .set_tool_rule(name, action, self.session_generation);
            }
            ViewAction::RefreshPlugins => self.client.load_plugins(self.session_generation),
            ViewAction::Plugin(operation) => self
                .client
                .plugin_operation(operation, self.session_generation),
            ViewAction::RefreshTelemetry => self.refresh_telemetry(),
            ViewAction::ToggleTelemetryScope => {
                self.model.telemetry_scope = match self.model.telemetry_scope {
                    crate::ui_model::TelemetryScope::Host => {
                        crate::ui_model::TelemetryScope::Session
                    }
                    crate::ui_model::TelemetryScope::Session => {
                        crate::ui_model::TelemetryScope::Host
                    }
                };
                self.refresh_telemetry();
            }
            ViewAction::RefreshCosts => self.refresh_costs(),
            ViewAction::RefreshJobs => self.refresh_jobs(),
            ViewAction::CompactSession => {
                if self.model.active_child.is_none()
                    && let Some(session) = self.model.active_session.clone()
                {
                    self.model
                        .transcript
                        .push(TranscriptItem::Status("Compaction requested...".to_owned()));
                    self.client
                        .compact_session(session, self.session_generation);
                }
            }
            ViewAction::ExportSession => {
                if self.model.active_child.is_none()
                    && let Some(session) = self.model.active_session.clone()
                {
                    self.model.notice = Some("Preparing session export...".to_owned());
                    self.client.export_session(session, self.session_generation);
                }
            }
            ViewAction::TogglePreference(preference) => self.toggle_preference(preference),
        }
    }

    fn send_message(&mut self, text: String, chips: &[crate::chips::Chip]) {
        if self.model.active_child.is_some()
            || self.model.attachment_state != SessionAttachmentState::Attached
            || (self.model.turn_state.is_busy() && self.model.send_mode == SendMode::Reject)
        {
            return;
        }
        let Some(session) = self.model.active_session.clone() else {
            return;
        };
        let parts = crate::chips::parts(chips);
        let queued = self.model.turn_state.is_busy();
        self.model.transcript.push(TranscriptItem::UserMessage {
            queued,
            text: text.clone(),
            chips: chips.to_vec(),
            turn_id: None,
        });
        self.model.turn_state = TurnState::Connecting;
        self.client.send_message(
            session,
            self.session_generation,
            text,
            parts,
            self.model.send_mode,
        );
    }

    /// Attaches a file to the next message.
    ///
    /// The host has to accept uploads and the agent has to accept the kind of
    /// file, and both say so before anything is read.
    fn attach_file(&mut self, path: String) {
        let Some(session) = self.model.active_session.clone() else {
            return;
        };
        if self.model.active_child.is_some() {
            return;
        }
        if !self.uploads_supported() {
            self.model.notice = Some(
                "This host does not accept uploads, so there is nothing to attach.".to_owned(),
            );
            return;
        }
        let path = std::path::PathBuf::from(path);
        let Some(mime) = norte_code_client::mime_of_path(&path) else {
            self.model.notice = Some(format!(
                "{} has no media type this client can send.",
                path.display()
            ));
            return;
        };
        let Some(kind) = norte_code_client::input_part_kind(mime) else {
            self.model.notice = Some(format!("{mime} is not a media type this client sends."));
            return;
        };
        let modality = norte_code_client::modality_of(kind);
        if !self.agent_accepts(modality) {
            self.model.notice = Some(format!(
                "This agent does not accept {} input.",
                norte_code_client::modality_word(modality)
            ));
            return;
        }
        self.client.upload_file(
            session,
            path,
            mime.to_owned(),
            kind,
            self.session_generation,
        );
    }

    /// Whether the host accepts uploads at all.
    fn uploads_supported(&self) -> bool {
        self.model.server.as_ref().is_some_and(|server| {
            server
                .capabilities
                .iter()
                .any(|capability| capability == UPLOADS_CAPABILITY)
        })
    }

    /// Whether the agent of the session accepts a modality as input.
    fn agent_accepts(&self, modality: norte_code_wire::OutputModality) -> bool {
        let Some(session) = self.model.active_session.as_ref() else {
            return false;
        };
        let agent_id = self
            .model
            .sessions
            .iter()
            .find(|summary| summary.session_id == session.as_str())
            .and_then(|summary| summary.agent.as_ref())
            .map(|agent| agent.id.clone());
        let Some(agent_id) = agent_id else {
            return false;
        };
        let Some(agent) = self.model.agents.iter().find(|agent| agent.id == agent_id) else {
            return false;
        };
        let Some(capabilities) = agent.capabilities.as_ref() else {
            return false;
        };
        capabilities.input.contains(&modality)
    }

    fn set_budget(&mut self, limit: f64, action: norte_code_wire::BudgetAction) {
        if limit.is_finite()
            && limit > 0.0
            && self.model.active_child.is_none()
            && let Some(session) = self.model.active_session.clone()
        {
            self.client
                .set_budget(session, limit, action, self.session_generation);
        }
    }

    fn approve_budget(&mut self) {
        if let (Some(session), Some(approval)) = (
            self.model.active_session.clone(),
            self.model.budget_approval.take(),
        ) && approval.scope == norte_code_wire::BudgetScope::Session
            && approval.unit == norte_code_wire::BudgetUnit::Usd
        {
            self.model.turn_state = TurnState::Connecting;
            self.model.transcript.push(TranscriptItem::Status(
                "Budget approved for one continuation.".to_owned(),
            ));
            self.client
                .approve_budget(session, approval.limit, self.session_generation);
        }
    }

    fn import_session(&mut self, path: String) {
        let Some(session) = self.model.active_session.clone() else {
            return;
        };
        if self.model.active_child.is_some() {
            return;
        }
        self.client.import_session(
            session,
            std::path::PathBuf::from(path),
            self.session_generation,
        );
    }

    fn decline_budget(&mut self) {
        if self.model.budget_approval.take().is_some() {
            self.model.transcript.push(TranscriptItem::Status(
                "Budget approval declined. The turn remains stopped.".to_owned(),
            ));
        }
    }

    fn cancel_turn(&mut self) {
        let (Some(session), Some(turn)) = (
            self.model.active_session.clone(),
            self.model.active_turn.clone(),
        ) else {
            self.model.notice = Some("There is no running turn to stop.".to_owned());
            return;
        };
        if self.model.active_child.is_some() {
            return;
        }
        self.model.turn_state = TurnState::Idle;
        self.client
            .cancel_turn(session, turn, self.session_generation);
    }

    fn set_reasoning(&mut self, level: crate::ui_model::ReasoningLevel) {
        let Some(session) = self.model.active_session.clone() else {
            return;
        };
        if self.model.active_child.is_some() {
            return;
        }
        if let Some(message) = self
            .current_model_info()
            .and_then(|model| level.incompatible_with(&model.reasoning))
        {
            self.model.notice = Some(format!("{message}; not changing reasoning."));
            return;
        }
        self.reasoning_recovery.clear();
        self.model.reasoning = level;
        self.client
            .set_reasoning(session, level, self.session_generation);
    }

    /// The reasoning capabilities of the selected model, when the client
    /// knows them.
    pub(super) fn catalog_capabilities(&self) -> Option<&norte_code_wire::ReasoningInfo> {
        self.current_model_info().map(|info| &info.reasoning)
    }

    /// The catalogue entry for the selected model, when the client knows it.
    fn current_model_info(&self) -> Option<&norte_code_wire::ModelInfo> {
        let (provider, model) = self.model.session_model.as_ref().map_or_else(
            || {
                self.model
                    .active_session
                    .as_ref()
                    .and_then(|session| {
                        self.model
                            .sessions
                            .iter()
                            .find(|summary| summary.session_id == session.as_str())
                    })
                    .and_then(|summary| {
                        Some((summary.provider.clone()?, summary.model.clone()?.id))
                    })
            },
            |model| Some((model.provider.clone(), model.model.clone())),
        )?;
        self.model
            .models
            .iter()
            .find(|info| info.provider == provider && info.model == model)
    }

    fn rewind(&mut self, turn_id: String) {
        let Some(session) = self.model.active_session.clone() else {
            return;
        };
        if self.model.active_child.is_some() {
            return;
        }
        self.model.turn_state = TurnState::Idle;
        self.client
            .rewind(session, turn_id, self.session_generation);
    }

    fn pin_turn(&mut self, turn_id: String, pinned: bool) {
        let Some(session) = self.model.active_session.clone() else {
            return;
        };
        if self.model.active_child.is_some() {
            return;
        }
        self.client
            .pin_turns(session, turn_id, pinned, self.session_generation);
    }

    fn stop_job(&mut self, job_id: String) {
        let Some(session) = self.model.active_session.clone() else {
            return;
        };
        if self.model.active_child.is_some() {
            return;
        }
        self.client
            .stop_job(session, job_id, self.session_generation);
    }

    fn answer_question(
        &mut self,
        session: norte_code_wire::SessionKey,
        question: String,
        answer: String,
    ) {
        if let Err(error) = self.plan_review.close() {
            self.model.notice = Some(error.to_string());
        }
        self.client
            .answer_question(session, self.session_generation, question, answer);
    }

    fn toggle_preference(&mut self, preference: Preference) {
        match preference {
            Preference::Theme => {
                self.model.preferences.theme = match self.model.preferences.theme {
                    ThemeMode::Dark => ThemeMode::Light,
                    ThemeMode::Light => ThemeMode::Dark,
                };
            }
            Preference::Thinking => {
                self.model.preferences.thinking = match self.model.preferences.thinking {
                    ThinkingVisibility::Collapsed => ThinkingVisibility::Expanded,
                    ThinkingVisibility::Expanded => ThinkingVisibility::Collapsed,
                }
            }
            Preference::Timestamps => {
                self.model.preferences.show_timestamps =
                    self.model.preferences.show_timestamps.toggled();
            }
            Preference::ToolOutput => {
                self.model.preferences.show_tool_output =
                    self.model.preferences.show_tool_output.toggled();
            }
            Preference::LiveRefresh => {
                self.model.preferences.auto_refresh_sessions =
                    self.model.preferences.auto_refresh_sessions.toggled();
                self.last_session_refresh = std::time::Instant::now();
            }
            Preference::DeveloperMode => {
                self.model.preferences.developer_mode =
                    self.model.preferences.developer_mode.toggled();
            }
            Preference::DiffStyle => {
                self.model.cli.diff_style = match self.model.cli.diff_style {
                    crate::tools::DiffMode::Unified => crate::tools::DiffMode::Split,
                    crate::tools::DiffMode::Split => crate::tools::DiffMode::Unified,
                };
            }
            Preference::BudgetAction => {
                self.model.budget_action = match self.model.budget_action {
                    norte_code_wire::BudgetAction::Warn => norte_code_wire::BudgetAction::Ask,
                    norte_code_wire::BudgetAction::Ask => norte_code_wire::BudgetAction::Stop,
                    norte_code_wire::BudgetAction::Stop => norte_code_wire::BudgetAction::Warn,
                };
            }
        }
        self.persist_preferences();
    }

    fn persist_preferences(&mut self) {
        self.config.update_preferences(&self.model);
        if let Err(error) = self.config.save() {
            self.model.notice = Some(format!("Could not save settings: {error}"));
        }
    }

    fn refresh_telemetry(&self) {
        let session = match self.model.telemetry_scope {
            crate::ui_model::TelemetryScope::Host => None,
            crate::ui_model::TelemetryScope::Session => self.model.active_session.clone(),
        };
        self.client.load_telemetry(session, self.session_generation);
    }

    fn refresh_costs(&self) {
        self.client.load_costs(None, self.session_generation);
        if let Some(session) = self.model.active_session.clone() {
            self.client
                .load_costs(Some(session.clone()), self.session_generation);
        }
    }

    /// Asks the host for the session's jobs, the way a page opened on them
    /// would like to see them fresh.
    fn refresh_jobs(&self) {
        if let Some(session) = self.model.active_session.clone() {
            self.client.load_jobs(session, self.session_generation);
        }
    }
}

#[cfg(test)]
#[path = "actions_tests.rs"]
mod tests;
