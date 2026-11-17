use norte_code_wire::{
    Budget, BudgetAction, BudgetScope, BudgetUnit, ChildSessionSummary, ChildState,
    ServerEventKind, SessionKey,
};

use super::{App, client_data::ErrorDestination};
use crate::{
    client::{ClientData, ClientEvent, ClientOperationError},
    components::{ComponentEvent, ComponentId},
    ui_model::{
        BudgetApprovalRequest, ReasoningLevel, SessionAttachmentState, SessionModel,
        TranscriptItem, TurnState,
    },
};

impl App {
    /// Whether this error killed the turn in progress: the turn was alive
    /// before it and died now. The happy path reloads the page when the
    /// turn finishes; the failing path deserves the same truth from the
    /// store when its turn dies.
    pub(super) fn is_turn_ending_error(
        kind: &ServerEventKind,
        state: crate::ui_model::TurnState,
    ) -> bool {
        matches!(kind, ServerEventKind::Error { .. }) && state.is_busy()
    }

    pub(super) fn drain_client_events(&mut self) -> Result<(), super::AppError> {
        while let Some(client_event) = self.client.try_recv() {
            self.apply_client_event(client_event)?;
        }
        Ok(())
    }

    /// Records a successful attach, and clears the reconnecting notice.
    fn apply_attached(&mut self) {
        self.model.attachment_state = SessionAttachmentState::Attached;
        let was_down = self.model.service_down;
        self.model.service_down = false;
        if was_down {
            self.model
                .transcript
                .push(TranscriptItem::Status("Service is back.".to_owned()));
        }
        if self
            .model
            .notice
            .as_deref()
            .is_some_and(|n| n.starts_with("Connection lost"))
        {
            self.model.notice = None;
        }
    }

    /// Records that the open stream went away.
    fn apply_reconnecting(&mut self, attempt: u32) {
        self.model.attachment_state = SessionAttachmentState::Connecting;
        self.model.notice = Some(format!(
            "Connection lost; reconnecting (attempt {}).",
            attempt + 1
        ));
    }

    /// Records a rewind that arrived from the host.
    fn apply_rewind(&mut self, removed: usize) {
        self.model
            .transcript
            .push(TranscriptItem::Status(Self::rewound_message(removed)));
        self.model.active_turn = None;
    }

    /// Applies one server event that arrived on the session's stream.
    fn apply_server_event(
        &mut self,
        session: norte_code_wire::SessionKey,
        generation: u64,
        event: norte_code_wire::ServerEvent,
    ) {
        if matches!(&event.kind, ServerEventKind::MessageReplayed { .. }) {
            return;
        }
        self.handle_server_event(session, generation, event);
    }

    /// Applies one data event that arrived from the host.
    fn apply_data_event(
        &mut self,
        session: Option<&SessionKey>,
        result: Box<Result<ClientData, ClientOperationError>>,
    ) -> Result<(), super::AppError> {
        let destination = if session.is_some() {
            ErrorDestination::Transcript
        } else {
            ErrorDestination::Notice
        };
        self.apply_client_data(*result, destination)
    }

    /// Keeps a pinned-turns list that arrived from the host.
    fn apply_pins(&mut self, pinned_turns: Vec<String>) {
        let count = pinned_turns.len();
        self.model.pinned_turns = pinned_turns;
        self.model
            .transcript
            .push(TranscriptItem::Status(Self::pinned_message(count)));
    }

    /// Opens a session that arrived from an import.
    fn apply_import(&mut self, session_id: String) {
        self.model.notice = Some("Session imported.".to_owned());
        self.open_session(norte_code_wire::SessionKey::new(session_id));
    }

    /// Records a failure that arrived for the session.
    ///
    /// An attachment that failed also leaves the session unattached, so the
    /// reader knows that what follows will not reach the host.
    fn apply_failure(&mut self, message: String, attachment_failed: bool) {
        self.model.turn_state = TurnState::Idle;
        if attachment_failed {
            self.model.attachment_state = SessionAttachmentState::Failed;
        }
        if Self::is_transport_failure(&message) {
            // The request never reached a host: one line tells the reader
            // why the transcript went quiet, and the retries that keep
            // failing while the service is down stay out of it.
            if !self.model.service_down {
                self.model.service_down = true;
                self.model
                    .transcript
                    .push(TranscriptItem::Error(Self::transport_notice(&message)));
            }
            return;
        }
        let recovered = self.maybe_recover_reasoning_from_message(&message);
        self.model.transcript.push(TranscriptItem::Error(message));
        if recovered {
            self.sync_reasoning_with_host();
        }
    }

    fn is_unsupported_effort_message(message: &str) -> bool {
        message.contains("does not support effort")
    }

    /// Whether the failure is the transport being down: the request never
    /// reached a host, so nothing about the session decided it.
    pub(super) fn is_transport_failure(message: &str) -> bool {
        message.contains("error sending request for url")
    }

    /// The one line a transport failure tells the reader, naming the host
    /// the request was meant for when the message carries its URL.
    pub(super) fn transport_notice(message: &str) -> String {
        let rest = message
            .split_once("url (")
            .map(|(_, rest)| rest.trim_end_matches(')'));
        let authority = rest
            .and_then(|url| url.split_once("://"))
            .map(|(_, rest)| rest)
            .and_then(|rest| rest.split(['/', '?']).next())
            .filter(|authority| !authority.is_empty());
        match authority {
            Some(host) => format!("Service unreachable ({host}); retrying in the background."),
            None => "Service unreachable; retrying in the background.".to_owned(),
        }
    }

    pub(super) fn maybe_recover_reasoning_from_message(&mut self, message: &str) -> bool {
        if !Self::is_unsupported_effort_message(message) {
            return false;
        }
        let refused = self.model.reasoning;
        if !self
            .reasoning_recovery
            .claim(self.session_generation, refused)
        {
            return false;
        }
        self.model.reasoning = ReasoningLevel::Automatic;
        if refused == ReasoningLevel::Automatic {
            self.model.transcript.push(TranscriptItem::Status(
                "The stored effort is not supported by this model; using auto.".to_owned(),
            ));
            return true;
        }
        self.model.transcript.push(TranscriptItem::Status(format!(
            "Reasoning effort {} is not supported by this model; using auto.",
            refused.label()
        )));
        true
    }

    pub(super) fn sync_reasoning_with_host(&mut self) {
        let Some(session) = self.model.active_session.clone() else {
            return;
        };
        self.client
            .set_reasoning(session, ReasoningLevel::Automatic, self.session_generation);
    }

    /// Restores a supported effort when the model refused one.
    fn apply_reasoning_error(&mut self, _code: norte_code_wire::ErrorCode, message: &str) {
        if self.maybe_recover_reasoning_from_message(message) {
            self.sync_reasoning_with_host();
        }
    }

    fn apply_reasoning_downgrade(&mut self, from: &str, to: &str, reason: &str) {
        let target = ReasoningLevel::from_wire_name(to).unwrap_or(ReasoningLevel::Automatic);
        if self.model.reasoning == target {
            return;
        }
        self.model.reasoning = target;
        if !reason.trim().is_empty() && !Self::is_unsupported_effort_message(reason) {
            self.model.transcript.push(TranscriptItem::Status(format!(
                "Reasoning changed from {from} to {}; {reason}.",
                target.label()
            )));
        }
        if target == ReasoningLevel::Automatic && Self::is_unsupported_effort_message(reason) {
            self.sync_reasoning_with_host();
        }
    }

    /// Applies one event that arrived from the host.
    fn apply_client_event(&mut self, client_event: ClientEvent) -> Result<(), super::AppError> {
        self.apply_client_event_inner(client_event)
    }

    fn apply_client_event_inner(
        &mut self,
        client_event: ClientEvent,
    ) -> Result<(), super::AppError> {
        match client_event {
            ClientEvent::Attached {
                session,
                generation,
            } if self.is_current_session_event(&session, generation) => {
                self.apply_attached();
            }
            ClientEvent::Server {
                session,
                generation,
                event,
            } if self.is_current_session_event(&session, generation) => {
                self.apply_server_event(session, generation, *event);
            }
            ClientEvent::Failure {
                session,
                generation,
                message,
            } if self.is_current_session_event(&session, generation) => {
                self.apply_failure(message, false);
            }
            ClientEvent::StreamClosed {
                session,
                generation,
                attempt,
            } if self.is_current_session_event(&session, generation) => {
                self.model.turn_state = TurnState::Idle;
                self.model.active_turn = None;
                self.apply_reconnecting(attempt);
            }
            ClientEvent::AttachmentFailure {
                session,
                generation,
                message,
            } if self.is_current_session_event(&session, generation) => {
                self.apply_failure(message, true);
            }
            ClientEvent::Rewound {
                session,
                generation,
                removed,
            } if self.is_current_session_event(&session, generation) => {
                self.apply_rewind(removed.len());
            }
            ClientEvent::Pinned {
                session,
                generation,
                pinned_turns,
            } if self.is_current_session_event(&session, generation) => {
                self.apply_pins(pinned_turns);
            }
            ClientEvent::JobStopped {
                session,
                generation,
                job,
            } if self.is_current_session_event(&session, generation) => {
                self.apply_finished_job(&job);
            }
            ClientEvent::Jobs {
                session,
                generation,
                jobs,
            } if self.is_current_session_event(&session, generation) => {
                self.apply_job_list(jobs);
            }
            ClientEvent::Uploaded {
                session,
                generation,
                chip,
            } if self.is_current_session_event(&session, generation) => {
                self.give_to_session_view(ComponentEvent::Uploaded(chip));
            }
            ClientEvent::Imported {
                session,
                generation,
                session_id,
            } if self.is_current_session_event(&session, generation) => {
                self.apply_import(session_id);
            }
            ClientEvent::Data {
                session,
                generation,
                result,
            } if self.is_current_data_event(session.as_ref(), generation) => {
                self.apply_data_event(session.as_ref(), result)?;
            }
            ClientEvent::Attached { .. }
            | ClientEvent::AttachmentFailure { .. }
            | ClientEvent::StreamClosed { .. }
            | ClientEvent::Server { .. }
            | ClientEvent::Failure { .. }
            | ClientEvent::Rewound { .. }
            | ClientEvent::Pinned { .. }
            | ClientEvent::Jobs { .. }
            | ClientEvent::JobStopped { .. }
            | ClientEvent::Uploaded { .. }
            | ClientEvent::Imported { .. }
            | ClientEvent::Data { .. } => {}
        }
        Ok(())
    }

    fn handle_server_event(
        &mut self,
        session: norte_code_wire::SessionKey,
        generation: u64,
        event: norte_code_wire::ServerEvent,
    ) {
        if let ServerEventKind::Error { code, message } = &event.kind {
            self.apply_reasoning_error(*code, message);
            if message.starts_with("RAEP sequence gap") {
                self.client.load_history(session, generation);
            }
        }
        if let ServerEventKind::Error { message, .. } = &event.kind
            && Self::is_transport_failure(message)
        {
            // A command that never reached the host: told once, like every
            // other transport failure, and quiet while the retries fail.
            if !self.model.service_down {
                self.model.service_down = true;
                self.model
                    .transcript
                    .push(TranscriptItem::Error(Self::transport_notice(message)));
            }
            return;
        }
        if let ServerEventKind::ReasoningDowngraded { from, to, reason } = &event.kind {
            self.apply_reasoning_downgrade(from, to, reason);
        }
        if let ServerEventKind::BudgetExceeded {
            scope,
            unit,
            spent,
            limit,
            action: BudgetAction::Ask,
        } = &event.kind
            && *scope == BudgetScope::Session
            && *unit == BudgetUnit::Usd
        {
            self.model.budget_approval = Some(BudgetApprovalRequest {
                scope: *scope,
                unit: *unit,
                spent: *spent,
                limit: *limit,
            });
        }
        self.open_question(&event.kind, event.session.as_ref());
        let refresh_plans = matches!(&event.kind, ServerEventKind::TurnFinished { .. })
            && self.model.active_child.is_none();
        let error_kills_the_turn = App::is_turn_ending_error(&event.kind, self.model.turn_state);
        self.model.turn_state.apply_event(&event.kind);
        self.update_child_state(&event.kind);
        self.track_turn(&event.kind);
        self.track_todos(&event.kind);
        self.track_model(&event.kind);
        self.track_job(&event.kind);
        self.track_session_stats(&event.kind);
        if (matches!(
            &event.kind,
            ServerEventKind::TurnFinished { .. } | ServerEventKind::TurnStopped { .. }
        ) || error_kills_the_turn)
            && self.model.active_child.is_none()
            && let Some(session) = self.model.active_session.clone()
        {
            self.client.load_history(session, self.session_generation);
        }
        if let Some(item) = Self::transcript_status(&event.kind) {
            self.model.transcript.push(item);
            return;
        }
        self.model
            .transcript
            .push(TranscriptItem::ServerEvent(Box::new(event)));
        if refresh_plans && let Some(session) = self.model.active_session.clone() {
            self.client.load_plans(session, self.session_generation);
        }
    }

    fn track_turn(&mut self, kind: &ServerEventKind) {
        match kind {
            ServerEventKind::InputAccepted { turn_id, .. } => {
                self.model.active_turn = Some(turn_id.clone());
                // The live copy of the message gets the same turn id the
                // history will use for it, so the optimistic transcript and
                // the canonical page can recognize each other.
                if let Some(position) = self.model.transcript.iter().position(|item| {
                    matches!(
                        item,
                        crate::ui_model::TranscriptItem::UserMessage { turn_id: None, .. }
                    )
                }) && let crate::ui_model::TranscriptItem::UserMessage { turn_id: tag, .. } =
                    &mut self.model.transcript[position]
                {
                    *tag = Some(turn_id.clone());
                }
            }
            ServerEventKind::TurnStarted { .. } => {
                // The next queued message just left the queue: what the
                // reader typed is being answered now, so it is not pending
                // anymore.
                self.model.transcript.iter_mut().for_each(|item| {
                    if let crate::ui_model::TranscriptItem::UserMessage { queued, .. } = item {
                        *queued = false;
                    }
                });
            }
            ServerEventKind::TurnFinished { .. } | ServerEventKind::TurnStopped { .. } => {
                self.model.active_turn = None;
            }
            _ => {}
        }
    }

    fn track_todos(&mut self, kind: &ServerEventKind) {
        if let Some(todos) = Self::todo_list(kind) {
            self.model.todos = todos;
        }
    }

    fn track_model(&mut self, kind: &ServerEventKind) {
        if let Some((model, message)) = Self::model_change(kind) {
            self.model.session_model = Some(model);
            self.model.queued_model = None;
            self.model.transcript.push(TranscriptItem::Status(message));
        } else if let Some((model, message)) = Self::model_change_pending(kind) {
            self.model.queued_model = Some(model);
            self.model.transcript.push(TranscriptItem::Status(message));
        }
    }

    /// The task list the host sent, when it sent one.
    fn todo_list(kind: &ServerEventKind) -> Option<Vec<norte_code_wire::TodoInfo>> {
        let ServerEventKind::SessionTodos { todos } = kind else {
            return None;
        };
        Some(todos.clone())
    }

    /// The model the session now runs on, when it changed.
    fn model_change(kind: &ServerEventKind) -> Option<(SessionModel, String)> {
        let ServerEventKind::ModelChanged { provider, model } = kind else {
            return None;
        };
        Some((
            SessionModel {
                provider: provider.clone(),
                model: model.clone(),
            },
            Self::model_changed_message(provider, model),
        ))
    }

    /// The notice for a model change that waits for the turn to end.
    fn model_change_pending(kind: &ServerEventKind) -> Option<(SessionModel, String)> {
        let ServerEventKind::ModelQueued { provider, model } = kind else {
            return None;
        };
        Some((
            SessionModel {
                provider: provider.clone(),
                model: model.clone(),
            },
            Self::model_queued_message(provider, model),
        ))
    }

    /// Applies one event's contribution to the session's usage.
    ///
    /// The resume snapshot replaces the accumulator; every request's own
    /// accounting adds to it. Returns whether the event carried any.
    fn session_stats_update(
        stats: &mut crate::ui_model::SessionStatsSummary,
        kind: &ServerEventKind,
    ) -> bool {
        match kind {
            ServerEventKind::SessionStats {
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_write_tokens,
                cost_usd,
                requests,
                context_used,
                context_window,
            } => {
                *stats = crate::ui_model::SessionStatsSummary {
                    requests: *requests,
                    input_tokens: *input_tokens,
                    output_tokens: *output_tokens,
                    cache_read_tokens: *cache_read_tokens,
                    cache_write_tokens: *cache_write_tokens,
                    cost_usd: *cost_usd,
                    context_used: *context_used,
                    context_window: *context_window,
                };
                true
            }
            ServerEventKind::RequestReconstructed {
                usage, cost_usd, ..
            } => {
                stats.requests = stats.requests.saturating_add(1);
                stats.input_tokens = stats
                    .input_tokens
                    .saturating_add(usage.input_tokens.unwrap_or(0));
                stats.output_tokens = stats
                    .output_tokens
                    .saturating_add(usage.output_tokens.unwrap_or(0));
                stats.cache_read_tokens = stats
                    .cache_read_tokens
                    .saturating_add(usage.cache_read_tokens.unwrap_or(0));
                stats.cache_write_tokens = stats
                    .cache_write_tokens
                    .saturating_add(usage.cache_write_tokens.unwrap_or(0));
                stats.cost_usd += usage.cost_usd.or(*cost_usd).unwrap_or(0.0);
                if let Some(input) = usage.input_tokens {
                    stats.context_used = input;
                }
                true
            }
            _ => false,
        }
    }

    /// Tracks the session's usage as the host reports it or the requests
    /// accumulate it.
    fn track_session_stats(&mut self, kind: &ServerEventKind) {
        let mut stats = self.model.session_stats.take().unwrap_or_default();
        if Self::session_stats_update(&mut stats, kind) {
            self.model.session_stats = Some(stats);
        }
    }

    fn transcript_status(kind: &ServerEventKind) -> Option<TranscriptItem> {
        match kind {
            ServerEventKind::CompactionStarted { .. } => {
                Some(TranscriptItem::Status("Compaction started...".to_owned()))
            }
            ServerEventKind::CompactionFinished {
                compacted: true, ..
            } => Some(TranscriptItem::Status("Session compacted.".to_owned())),
            ServerEventKind::CompactionFinished {
                compacted: false,
                error: Some(error),
                ..
            } => Some(TranscriptItem::Error(format!("Compaction failed: {error}"))),
            ServerEventKind::CompactionFinished {
                compacted: false, ..
            } => Some(TranscriptItem::Status(
                "No compaction was needed.".to_owned(),
            )),
            ServerEventKind::BudgetChanged { budget } => Some(TranscriptItem::Status(
                Self::budget_changed(budget.as_ref()),
            )),
            ServerEventKind::BudgetWarning {
                scope,
                unit,
                spent,
                limit,
            } => Some(TranscriptItem::Status(format!(
                "Budget warning: {} spent of {} for {}.",
                Self::budget_amount(*spent, *unit),
                Self::budget_amount(*limit, *unit),
                Self::budget_scope(*scope)
            ))),
            ServerEventKind::BudgetExceeded {
                scope,
                unit,
                spent,
                limit,
                action,
            } => Some(TranscriptItem::Status(format!(
                "Budget reached: {} spent of {} for {}; {}.",
                Self::budget_amount(*spent, *unit),
                Self::budget_amount(*limit, *unit),
                Self::budget_scope(*scope),
                Self::budget_action(*action)
            ))),
            _ => None,
        }
    }

    fn budget_changed(budget: Option<&Budget>) -> String {
        budget.map_or_else(
            || "Session budget cleared.".to_owned(),
            |budget| {
                format!(
                    "Session budget set to {} for this session; {}.",
                    Self::budget_amount(budget.limit, budget.unit),
                    Self::budget_action(budget.action)
                )
            },
        )
    }

    fn budget_amount(value: f64, unit: BudgetUnit) -> String {
        match unit {
            BudgetUnit::Usd => format!("${value:.2}"),
            BudgetUnit::Tokens => format!("{value:.0} tokens"),
            BudgetUnit::Requests => format!("{value:.0} requests"),
        }
    }

    fn budget_scope(scope: BudgetScope) -> &'static str {
        match scope {
            BudgetScope::Message => "message",
            BudgetScope::Session => "session",
            BudgetScope::GlobalDaily => "daily total",
        }
    }

    fn budget_action(action: BudgetAction) -> &'static str {
        match action {
            BudgetAction::Warn => "warning only, continues",
            BudgetAction::Ask => "requires approval",
            BudgetAction::Stop => "stops the turn",
        }
    }

    fn rewound_message(removed: usize) -> String {
        match removed {
            0 => "Rewound: no turn was removed.".to_owned(),
            1 => "Rewound: one turn was removed.".to_owned(),
            count => format!("Rewound: {count} turns were removed."),
        }
    }

    fn pinned_message(pinned: usize) -> String {
        match pinned {
            0 => "No turn is pinned.".to_owned(),
            1 => "One turn is pinned.".to_owned(),
            count => format!("{count} turns are pinned."),
        }
    }

    fn model_changed_message(provider: &str, model: &str) -> String {
        format!("Model changed to {provider}/{model}.")
    }

    fn model_queued_message(provider: &str, model: &str) -> String {
        format!("Model change to {provider}/{model} applies after this turn.")
    }

    fn open_question(&mut self, kind: &ServerEventKind, session: Option<&SessionKey>) {
        if let ServerEventKind::QuestionAsked {
            question,
            prompt,
            options,
            multiple,
            ..
        } = kind
            && let Some(session) = session
            && let Err(error) =
                self.plan_review
                    .begin(session.clone(), question, prompt, options, *multiple)
        {
            self.model.notice = Some(error.to_string());
        }
    }

    fn is_current_session_event(&self, session: &SessionKey, generation: u64) -> bool {
        generation == self.session_generation && self.model.active_session.as_ref() == Some(session)
    }

    /// Gives something that arrived to the session view.
    pub(super) fn give_to_session_view(&mut self, event: ComponentEvent) {
        let Some(index) = self.view_index(ComponentId::Session) else {
            return;
        };
        if let Some(view) = self.views.get_mut(index) {
            view.accept(event);
        }
    }

    fn is_current_data_event(&self, session: Option<&SessionKey>, generation: u64) -> bool {
        session.is_none()
            || (generation == self.session_generation
                && session.is_some_and(|key| self.model.active_session.as_ref() == Some(key)))
    }

    fn update_child_state(&mut self, kind: &ServerEventKind) {
        match kind {
            ServerEventKind::ChildSessionStarted {
                child_session_id,
                agent_id,
                task_summary,
                ..
            } => {
                if !self
                    .model
                    .children
                    .iter()
                    .any(|child| child.session_id == *child_session_id)
                {
                    self.model.children.push(ChildSessionSummary {
                        session_id: child_session_id.clone(),
                        state: ChildState::Running,
                        agent: Some(agent_id.clone()),
                        task: Some(task_summary.clone()),
                        ..ChildSessionSummary::default()
                    });
                }
            }
            ServerEventKind::ChildSessionUpdated {
                child_session_id,
                state,
                ..
            } => {
                if let Some(child) = self
                    .model
                    .children
                    .iter_mut()
                    .find(|child| child.session_id == *child_session_id)
                {
                    child.state = *state;
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "client_events_tests.rs"]
mod tests;
