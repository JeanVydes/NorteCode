use crate::api::ServerInfo;
use crate::domain::{
    ChatState, ConnectionState, ConversationItem, Page, PendingQuestion, QueueMode, ServerProfile,
    SessionTodo, ThemeMode,
};
use dioxus::prelude::{ReadableExt, Signal, WritableExt};
use norte_code_wire::JobSummary;
use norte_code_wire::{AgentSummary, SessionSummary, TelemetryReport};
use raep_types::TurnId;

#[derive(Clone, Copy)]
pub struct AppContext {
    pub page: Signal<Page>,
    pub profiles: Signal<Vec<ServerProfile>>,
    pub active_profile_id: Signal<String>,
    pub connection: Signal<ConnectionState>,
    pub connection_error: Signal<Option<String>>,
    pub server_info: Signal<Option<ServerInfo>>,
    pub agents: Signal<Vec<AgentSummary>>,
    pub sessions: Signal<Vec<SessionSummary>>,
    pub selected_session: Signal<Option<SessionSummary>>,
    pub chat_items: Signal<Vec<ConversationItem>>,
    pub session_todos: Signal<Vec<SessionTodo>>,
    pub pinned_turns: Signal<Vec<String>>,
    pub pending_questions: Signal<Vec<PendingQuestion>>,
    pub chat_state: Signal<ChatState>,
    pub active_turn_id: Signal<Option<TurnId>>,
    /// The turn the composer will steer into (the running one), kept apart
    /// from `active_turn_id` so a cancelled turn still has its id for the
    /// server to look up.
    pub streaming_turn_id: Signal<Option<String>>,
    /// Bumped whenever the conversation page must open a fresh stream, for
    /// example after a rewind or when the user opens another session.
    pub conversation_epoch: Signal<u64>,
    pub queue_mode: Signal<QueueMode>,
    /// Child sessions of the open session, kept live so the Subagents tab
    /// reflects a spawn the moment it happens.
    pub children: Signal<Vec<norte_code_wire::ChildSessionSummary>>,
    /// Whether a compaction is running for the open session.
    pub compacting: Signal<bool>,
    /// Turns this page accepted but that are still waiting behind the running
    /// one, in arrival order. A queued turn must keep the page "working" once
    /// the turn in front of it ends.
    pub pending_turns: Signal<std::collections::VecDeque<String>>,
    /// Sessions deleted in this session of the app. The list is refreshed by
    /// polling, and a reply that was in flight when the delete landed must not
    /// put the row back.
    pub deleted_sessions: Signal<std::collections::HashSet<String>>,
    /// Whether the sidebar refreshes its session list on a timer.
    ///
    /// The list is the only place another client's activity is visible, so the
    /// poll exists; but a tab that is not being looked at has no reason to ask
    /// every few seconds, and a person on a metered connection may prefer none
    /// at all.
    pub live_session_list: Signal<bool>,
    /// The newest history request, by number.
    ///
    /// Two turn ends close together, a job's turn and the auto-turn that
    /// follows it, start two history reads, and the replies come back in
    /// whatever order the network gives them. Applying the older reply over
    /// the newer one dropped the answer the person just watched arrive; the
    /// text only came back on a reload. A reply applies only if it is still
    /// the newest request.
    pub history_request: Signal<u64>,
    pub jobs: Signal<Vec<JobSummary>>,
    pub telemetry: Signal<Option<TelemetryReport>>,
    pub page_error: Signal<Option<String>>,
    pub theme: Signal<ThemeMode>,
    pub show_thinking: Signal<bool>,
    pub show_timestamps: Signal<bool>,
    pub show_tool_output: Signal<bool>,
    pub developer_mode: Signal<bool>,
}

impl AppContext {
    pub fn active_profile(self) -> Option<ServerProfile> {
        let id = self.active_profile_id.read();
        self.profiles
            .read()
            .iter()
            .find(|profile| profile.id == *id)
            .cloned()
    }

    pub fn set_page(self, value: Page) {
        let mut signal = self.page;
        signal.set(value);
    }

    pub fn set_connection(self, value: ConnectionState) {
        let mut signal = self.connection;
        signal.set(value);
    }

    pub fn set_connection_error(self, value: Option<String>) {
        let mut signal = self.connection_error;
        signal.set(value);
    }

    pub fn set_server_info(self, value: Option<ServerInfo>) {
        let mut signal = self.server_info;
        signal.set(value);
    }

    pub fn set_agents(self, value: Vec<AgentSummary>) {
        let mut signal = self.agents;
        signal.set(value);
    }

    pub fn set_sessions(self, value: Vec<SessionSummary>) {
        // A session deleted here must not come back: a list that was already
        // in flight when the delete landed would otherwise restore it, which
        // is what made deleted sessions reappear until the next refresh.
        let deleted = self.deleted_sessions.read().clone();
        let live = value
            .into_iter()
            .filter(|session| !deleted.contains(&session.session_id))
            .collect::<Vec<_>>();
        let still_there = self
            .selected_session
            .read()
            .as_ref()
            .is_none_or(|selected| {
                live.iter()
                    .any(|session| session.session_id == selected.session_id)
            });
        let missing = self
            .selected_session
            .read()
            .as_ref()
            .map(|selected| selected.session_id.clone())
            .filter(|_| !still_there);
        let mut signal = self.sessions;
        signal.set(live);
        // The selection is only as good as the list: a session the host no
        // longer lists must not stay open, because every request for it fails
        // until the page is reloaded.
        if let Some(missing) = missing {
            self.forget_session_without_tombstone(&missing);
        }
    }

    pub fn set_live_session_list(self, value: bool) {
        let mut signal = self.live_session_list;
        signal.set(value);
    }

    /// Claims the next history request number.
    pub fn next_history_request(self) -> u64 {
        let mut signal = self.history_request;
        let next = signal.peek().saturating_add(1);
        signal.set(next);
        next
    }

    /// Whether `request` is still the newest history request.
    pub fn is_latest_history_request(self, request: u64) -> bool {
        *self.history_request.peek() == request
    }

    /// Records a deletion so no later list can bring it back.
    pub fn mark_session_deleted(self, session_id: &str) {
        let mut deleted = self.deleted_sessions;
        deleted.with_mut(|ids| {
            ids.insert(session_id.to_owned());
        });
    }

    pub fn update_sessions(self, update: impl FnOnce(&mut Vec<SessionSummary>)) {
        let deleted = self.deleted_sessions.read().clone();
        let mut signal = self.sessions;
        signal.with_mut(|sessions| {
            update(sessions);
            sessions.retain(|session| !deleted.contains(&session.session_id));
        });
    }

    /// Drops a session the host no longer has: from the list, from the
    /// selection, and from the open conversation.
    ///
    /// Without this, a deleted session stayed selected in memory and every
    /// later attempt to open it failed with the host's "not found", the page
    /// looked broken until a reload cleared the id.
    pub fn forget_session(self, session_id: &str) {
        self.mark_session_deleted(session_id);
        self.forget_session_without_tombstone(session_id);
    }

    /// Drops the session from the list and the selection, without remembering
    /// the deletion.
    ///
    /// A list refresh that does not carry a session yet is not proof it is
    /// gone, a poll reply can be older than the create that just landed, so
    /// that path clears the selection and leaves the tombstone alone.
    pub fn forget_session_without_tombstone(self, session_id: &str) {
        self.update_sessions(|sessions| {
            sessions.retain(|session| session.session_id != session_id);
        });
        let was_selected = self
            .selected_session
            .read()
            .as_ref()
            .is_some_and(|session| session.session_id == session_id);
        if !was_selected {
            return;
        }
        let next = self.sessions.read().first().cloned();
        self.reset_conversation();
        if let Some(next) = next {
            self.select_session(next, Page::Chat);
        } else {
            self.set_selected_session(None);
            self.set_page(Page::Overview);
        }
    }

    pub fn set_selected_session(self, value: Option<SessionSummary>) {
        let mut signal = self.selected_session;
        signal.set(value);
    }

    /// Opens a session's conversation: clears the previous one's state and
    /// asks the page to open a fresh event stream (which is the attachment).
    pub fn select_session(self, session: SessionSummary, page: Page) {
        let is_current = self
            .selected_session
            .read()
            .as_ref()
            .is_some_and(|selected| selected.session_id == session.session_id);
        self.set_selected_session(Some(session));
        if !is_current {
            self.reset_conversation();
        }
        self.set_page(page);
    }

    pub fn reset_conversation(self) {
        self.set_chat_items(Vec::new());
        self.set_children(Vec::new());
        self.set_compacting(false);
        self.clear_pending_turns();
        self.set_session_todos(Vec::new());
        self.set_pinned_turns(Vec::new());
        self.set_pending_questions(Vec::new());
        self.set_jobs(Vec::new());
        self.set_active_turn_id(None);
        self.set_streaming_turn_id(None);
        self.set_chat_state(ChatState::Idle);
        self.bump_conversation_epoch();
    }

    pub fn bump_conversation_epoch(self) {
        let mut signal = self.conversation_epoch;
        let next = *signal.read() + 1;
        signal.set(next);
    }

    pub fn set_chat_items(self, value: Vec<ConversationItem>) {
        let mut signal = self.chat_items;
        signal.set(value);
    }

    pub fn update_chat_items(self, update: impl FnOnce(&mut Vec<ConversationItem>)) {
        let mut signal = self.chat_items;
        signal.with_mut(update);
    }

    pub fn set_session_todos(self, value: Vec<SessionTodo>) {
        let mut signal = self.session_todos;
        signal.set(value);
    }

    pub fn set_pinned_turns(self, value: Vec<String>) {
        let mut signal = self.pinned_turns;
        signal.set(value);
    }

    pub fn set_pending_questions(self, value: Vec<PendingQuestion>) {
        let mut signal = self.pending_questions;
        signal.set(value);
    }

    pub fn update_pending_questions(self, update: impl FnOnce(&mut Vec<PendingQuestion>)) {
        let mut signal = self.pending_questions;
        signal.with_mut(update);
    }

    pub fn set_chat_state(self, value: ChatState) {
        let mut signal = self.chat_state;
        signal.set(value);
    }

    pub fn set_active_turn_id(self, value: Option<TurnId>) {
        let mut signal = self.active_turn_id;
        signal.set(value);
    }

    pub fn set_streaming_turn_id(self, value: Option<String>) {
        let mut signal = self.streaming_turn_id;
        signal.set(value);
    }

    pub fn set_queue_mode(self, value: QueueMode) {
        let mut signal = self.queue_mode;
        signal.set(value);
    }

    pub fn set_children(self, value: Vec<norte_code_wire::ChildSessionSummary>) {
        let mut signal = self.children;
        signal.set(value);
    }

    /// Adds a child, or replaces the one with the same id.
    pub fn upsert_child(self, child: norte_code_wire::ChildSessionSummary) {
        let mut signal = self.children;
        signal.with_mut(|children| {
            if let Some(existing) = children
                .iter_mut()
                .find(|existing| existing.session_id == child.session_id)
            {
                *existing = child;
            } else {
                children.insert(0, child);
            }
        });
    }

    pub fn upsert_child_state(self, session_id: &str, state: norte_code_wire::ChildState) {
        let mut signal = self.children;
        signal.with_mut(|children| {
            if let Some(existing) = children
                .iter_mut()
                .find(|existing| existing.session_id == session_id)
            {
                existing.state = state;
            }
        });
    }

    /// Records a turn the server accepted, unless it is already running.
    pub fn add_pending_turn(self, turn_id: String) {
        let mut signal = self.pending_turns;
        signal.with_mut(|turns| {
            if !turns.contains(&turn_id) {
                turns.push_back(turn_id);
            }
        });
    }

    pub fn take_next_pending_turn(self) -> Option<String> {
        let mut signal = self.pending_turns;
        let mut turns = signal.write();
        turns.pop_front()
    }

    pub fn clear_pending_turns(self) {
        let mut signal = self.pending_turns;
        signal.set(std::collections::VecDeque::new());
    }

    /// Forgets a turn that ended, wherever it sat in the queue.
    pub fn complete_turn(self, turn_id: Option<&str>) {
        let Some(turn_id) = turn_id else {
            return;
        };
        let mut signal = self.pending_turns;
        signal.with_mut(|turns| turns.retain(|queued| queued != turn_id));
    }

    pub fn set_compacting(self, value: bool) {
        let mut signal = self.compacting;
        signal.set(value);
    }

    pub fn is_compacting(self) -> bool {
        *self.compacting.read()
    }

    /// Adds a line to the transcript's own log, compaction, rewind and other
    /// events that are not a message from either side but explain what just
    /// happened to the conversation.
    pub fn push_system_note(self, text: String) {
        let mut signal = self.chat_items;
        signal.with_mut(|items| {
            let timestamp = std::sync::atomic::AtomicI64::new(0);
            let seq = timestamp.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            items.push(ConversationItem {
                id: format!("note:{seq}:{}", items.len()),
                turn_id: None,
                role: crate::domain::HistoryRole::SystemEvent,
                kind: crate::domain::HistoryKind::SystemEvent,
                content: text,
                created_at: 0,
                delivery: crate::domain::MessageDelivery::Sent,
                tool: None,
                usage: None,
            });
        });
    }

    pub fn set_selected_model(self, model: String) {
        let mut signal = self.selected_session;
        signal.with_mut(|session| {
            if let Some(session) = session {
                session.model = Some(norte_code_wire::AgentRef {
                    id: model,
                    version: String::new(),
                });
            }
        });
    }

    pub fn set_jobs(self, value: Vec<JobSummary>) {
        let mut signal = self.jobs;
        signal.set(value);
    }

    pub fn set_telemetry(self, value: Option<TelemetryReport>) {
        let mut signal = self.telemetry;
        signal.set(value);
    }

    pub fn set_page_error(self, value: Option<String>) {
        let mut signal = self.page_error;
        signal.set(value);
    }

    pub fn set_theme(self, value: ThemeMode) {
        let mut signal = self.theme;
        signal.set(value);
    }

    pub fn set_show_thinking(self, value: bool) {
        let mut signal = self.show_thinking;
        signal.set(value);
    }

    pub fn set_show_timestamps(self, value: bool) {
        let mut signal = self.show_timestamps;
        signal.set(value);
    }

    pub fn set_show_tool_output(self, value: bool) {
        let mut signal = self.show_tool_output;
        signal.set(value);
    }

    pub fn set_developer_mode(self, value: bool) {
        let mut signal = self.developer_mode;
        signal.set(value);
    }

    pub fn set_active_profile_id(self, value: String) {
        let mut signal = self.active_profile_id;
        signal.set(value);
    }

    pub fn update_profiles(self, update: impl FnOnce(&mut Vec<ServerProfile>)) {
        let mut signal = self.profiles;
        signal.with_mut(update);
    }
}
