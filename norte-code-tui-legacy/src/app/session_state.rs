use norte_code_wire::SessionKey;

use super::App;
use crate::{
    components::ComponentId,
    ui_model::{SessionAttachmentState, TelemetryScope, TurnState},
};

use crate::ui_model::TurnPoint;

/// The user turns a reader can rewind to or pin.
pub(super) fn turn_points(history: &[norte_code_wire::HistoryItem]) -> Vec<TurnPoint> {
    history
        .iter()
        .filter(|item| item.role == norte_code_wire::HistorySpeaker::User)
        .filter_map(|item| {
            let turn_id = item.turn_id.clone()?;
            Some(TurnPoint {
                turn_id,
                preview: preview(&item.content),
            })
        })
        .collect()
}

/// The first line of a message, short enough to read in a list.
fn preview(content: &str) -> String {
    const LIMIT: usize = 60;
    let first_line = content.lines().next().unwrap_or_default().trim();
    if first_line.chars().count() <= LIMIT {
        return first_line.to_owned();
    }
    let mut short = first_line.chars().take(LIMIT).collect::<String>();
    short.push('…');
    short
}

impl App {
    pub(super) fn replace_history(
        &mut self,
        history: Vec<norte_code_wire::HistoryItem>,
        preserve_live: bool,
    ) {
        self.model.turn_points = turn_points(&history);
        let current = std::mem::take(&mut self.model.transcript);
        let preserve_live = preserve_live
            && super::actions::keeps_live_items(self.model.turn_state, &history, &current);
        self.model.transcript =
            super::actions::transcript_with_history(history, current, preserve_live);
    }

    /// Opens a session from a clean state.
    ///
    /// Everything the previous session left behind is dropped, so the reader
    /// starts with nothing but the session they asked for.
    pub(super) fn open_session(&mut self, session: SessionKey) {
        self.session_generation = self.session_generation.saturating_add(1);
        let summary = self
            .model
            .sessions
            .iter()
            .find(|summary| summary.session_id == session.as_str());
        let agent_id = summary
            .and_then(|summary| summary.agent.as_ref())
            .map(|agent| agent.id.clone());
        let reasoning = summary
            .and_then(|summary| summary.reasoning.as_ref())
            .map(crate::ui_model::ReasoningLevel::from_wire)
            .unwrap_or_default();
        self.model.active_session = Some(session.clone());
        self.model.active_child = None;
        self.model.attachment_state = SessionAttachmentState::Connecting;
        self.model.transcript.clear();
        self.model.plans.clear();
        self.model.children.clear();
        self.model.todos.clear();
        self.model.jobs.clear();
        self.model.turn_points.clear();
        self.model.pinned_turns.clear();
        self.model.active_turn = None;
        self.model.budget_approval = None;
        self.model.session_model = None;
        self.model.session_costs = None;
        self.model.session_stats = None;
        self.model.notice = None;
        self.model.clock.reset();
        self.model.reasoning = reasoning;
        self.model.queued_model = None;
        self.model.turn_state = TurnState::Idle;
        self.model.service_down = false;
        self.reasoning_recovery.clear();
        self.give_to_session_view(crate::components::ComponentEvent::SessionOpened);
        self.client
            .resume(&session, self.session_generation, agent_id.clone());
        self.client
            .load_plans(session.clone(), self.session_generation);
        self.client
            .load_children(session.clone(), self.session_generation);
        self.client
            .load_costs(Some(session.clone()), self.session_generation);
        self.client.load_jobs(session, self.session_generation);
        if self.model.telemetry_scope == TelemetryScope::Session {
            self.client
                .load_telemetry(self.model.active_session.clone(), self.session_generation);
        }
    }

    pub(super) fn open_child_session(&mut self, session: SessionKey) {
        if self.model.active_child.as_ref() == Some(&session) {
            return;
        }
        self.session_generation = self.session_generation.saturating_add(1);
        let agent = self
            .model
            .children
            .iter()
            .find(|child| child.session_id == session.as_str())
            .and_then(|child| child.agent.clone());
        self.model.active_session = Some(session.clone());
        self.model.active_child = Some(session.clone());
        self.model.attachment_state = SessionAttachmentState::Connecting;
        self.model.transcript.clear();
        self.model.todos.clear();
        self.model.turn_points.clear();
        self.model.pinned_turns.clear();
        self.model.active_turn = None;
        self.model.turn_state = TurnState::Idle;
        self.client
            .load_child_history(session.clone(), agent.clone(), self.session_generation);
        self.client
            .load_costs(Some(session), self.session_generation);
        if self.model.telemetry_scope == TelemetryScope::Session {
            self.client
                .load_telemetry(self.model.active_session.clone(), self.session_generation);
        }
    }

    pub(super) fn apply_created_session(
        &mut self,
        session: norte_code_wire::SessionSummary,
    ) -> Result<(), super::AppError> {
        let session_id = session.session_id.clone();
        self.model
            .sessions
            .retain(|existing| existing.session_id != session_id);
        self.model.sessions.insert(0, session);
        self.open_session(SessionKey::new(session_id));
        self.activate_view(ComponentId::Session)
    }

    pub(super) fn apply_sessions_updated(
        &mut self,
        sessions: Vec<norte_code_wire::SessionSummary>,
    ) {
        self.model.sessions = sessions;
        let active_is_missing = self.model.active_child.is_none()
            && self.model.active_session.as_ref().is_some_and(|active| {
                !self
                    .model
                    .sessions
                    .iter()
                    .any(|session| session.session_id == active.as_str())
            });
        if active_is_missing {
            self.model.active_session = None;
            self.model.active_child = None;
            self.model.attachment_state = SessionAttachmentState::Disconnected;
            self.model.transcript.clear();
            self.model.turn_state = TurnState::Idle;
            if let Some(session) = self.model.sessions.first() {
                self.open_session(SessionKey::new(session.session_id.clone()));
            }
        }
        self.model.notice = None;
        self.model.clock.reset();
    }
}
