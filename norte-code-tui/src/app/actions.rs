use norte_code_wire::{QueueMode, ServerEventKind};

use super::App;
use crate::{
    components::{ComponentAction, McpState, Preference, ViewAction},
    ui_model::{SessionAttachmentState, ThemeMode, ThinkingVisibility, TranscriptItem, TurnState},
};

/// The capability a host advertises when it accepts uploads.
const UPLOADS_CAPABILITY: &str = "uploads";

pub(super) fn transcript_with_history(
    history: Vec<norte_code_wire::HistoryItem>,
    current: Vec<TranscriptItem>,
    preserve_live: bool,
) -> Vec<TranscriptItem> {
    let mut transcript = history
        .into_iter()
        .map(TranscriptItem::History)
        .collect::<Vec<_>>();
    for item in current {
        if matches!(&item, TranscriptItem::Status(_)) {
            transcript.push(item);
            continue;
        }
        if !preserve_live {
            continue;
        }
        match &item {
            TranscriptItem::History(_) => continue,
            TranscriptItem::ServerEvent(event)
                if matches!(&event.kind, ServerEventKind::MessageReplayed { .. }) =>
            {
                continue;
            }
            TranscriptItem::UserMessage { text, .. }
                if transcript.iter().any(|entry| {
                    matches!(entry, TranscriptItem::History(history)
                        if history.role == norte_code_wire::HistorySpeaker::User
                            && history.content == *text)
                }) =>
            {
                continue;
            }
            _ => {}
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
            ComponentAction::SetQueueMode(mode) => {
                self.model.queue_mode = mode;
            }
            ComponentAction::SetModel { provider, model } => {
                if let Some(session) = self.model.active_session.clone()
                    && self.model.active_child.is_none()
                {
                    self.client
                        .set_model(session, self.session_generation, provider, model);
                }
            }
            ComponentAction::SetBudget { limit, action } => self.set_budget(limit, action),
            ComponentAction::ApproveBudget => self.approve_budget(),
            ComponentAction::DeclineBudget => self.decline_budget(),
            ComponentAction::CancelTurn => self.cancel_turn(),
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
            ViewAction::RefreshCatalog => self.client.load_catalog(self.session_generation),
            ViewAction::Catalog(operation) => self
                .client
                .catalog_operation(*operation, self.session_generation),
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
            || (self.model.turn_state.is_busy() && self.model.queue_mode == QueueMode::Reject)
        {
            return;
        }
        let Some(session) = self.model.active_session.clone() else {
            return;
        };
        let parts = crate::chips::parts(chips);
        self.model.transcript.push(TranscriptItem::UserMessage {
            text: text.clone(),
            chips: chips.to_vec(),
        });
        self.model.turn_state = TurnState::Connecting;
        self.client.send_message(
            session,
            self.session_generation,
            text,
            parts,
            self.model.queue_mode,
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
        self.model.reasoning = level;
        self.client
            .set_reasoning(session, level, self.session_generation);
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
}

#[cfg(test)]
#[path = "actions_tests.rs"]
mod tests;
