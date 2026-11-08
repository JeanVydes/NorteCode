use crate::api::JobSummary;
use crate::api::{AgentSummary, ServerInfo, SessionSummary, TelemetryReport};
use crate::domain::{ChatState, ConnectionState, ConversationItem, Page, ServerProfile, ThemeMode};
use dioxus::prelude::{ReadableExt, Signal, WritableExt};

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
    pub chat_state: Signal<ChatState>,
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
        let mut signal = self.sessions;
        signal.set(value);
    }

    pub fn update_sessions(self, update: impl FnOnce(&mut Vec<SessionSummary>)) {
        let mut signal = self.sessions;
        signal.with_mut(update);
    }

    pub fn set_selected_session(self, value: Option<SessionSummary>) {
        let mut signal = self.selected_session;
        signal.set(value);
    }

    pub fn set_chat_items(self, value: Vec<ConversationItem>) {
        let mut signal = self.chat_items;
        signal.set(value);
    }

    pub fn set_chat_state(self, value: ChatState) {
        let mut signal = self.chat_state;
        signal.set(value);
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
