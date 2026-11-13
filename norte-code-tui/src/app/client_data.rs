use norte_code_wire::AgentRef;

use super::App;
use crate::{
    client::{ClientData, ClientOperationError},
    ui_model::TranscriptItem,
};

#[derive(Clone, Copy)]
pub(super) enum ErrorDestination {
    Transcript,
    Notice,
}

impl App {
    pub(super) fn apply_client_data(
        &mut self,
        result: Result<ClientData, ClientOperationError>,
        error_destination: ErrorDestination,
    ) -> Result<(), super::AppError> {
        match result {
            Ok(data) => self.apply_client_result(data)?,
            Err(error) => self.apply_client_error(&error, error_destination),
        }
        Ok(())
    }

    fn apply_client_result(&mut self, data: ClientData) -> Result<(), super::AppError> {
        match data {
            ClientData::Models(models) => self.model.models = models,
            ClientData::Catalog(catalog) => {
                self.model.catalog = catalog;
                self.model.notice = None;
            }
            ClientData::HostFailure { probe, message } => {
                self.clear_host_error(probe);
                self.model.host_errors.push((probe.to_owned(), message));
            }
            ClientData::ServerInfo(server) => {
                self.clear_host_error("server info");
                self.model.server = Some(server);
            }
            ClientData::Liveness(liveness) => {
                self.clear_host_error("liveness");
                self.model.liveness = Some(liveness);
            }
            ClientData::Readiness(readiness) => {
                self.clear_host_error("readiness");
                self.model.readiness = Some(readiness);
            }
            ClientData::Quota(quota) => {
                self.clear_host_error("quota");
                self.model.quota = Some(quota);
            }
            ClientData::Agents(agents) => {
                self.clear_host_error("agents");
                self.model.agents = agents;
            }
            ClientData::Skills(skills) => {
                self.clear_host_error("skills");
                self.model.skills = Some(skills);
            }
            ClientData::Mcp(mcp) => {
                self.clear_host_error("MCP");
                self.model.mcp = Some(mcp);
            }
            ClientData::ToolRules(rules) => {
                self.clear_host_error("tool rules");
                self.model.tool_rules = rules;
            }
            ClientData::PluginUpdate { change, plugins } => {
                self.model.plugins = plugins;
                if !change.operation.is_empty() {
                    self.model.notice = Some(format!("{} {}", change.operation, change.name));
                }
            }
            ClientData::Telemetry(report) => self.model.telemetry = Some(report),
            ClientData::Costs { session, report } => {
                if session.is_some() {
                    self.model.session_costs = Some(report);
                } else {
                    self.model.costs = Some(report);
                }
            }
            ClientData::CompactedHistory(history) => {
                self.replace_history(history);
                self.model.notice = None;
            }
            ClientData::ExportReady(url) => {
                self.model.notice = Some(format!("Export ready: {url}"));
            }
            ClientData::SessionCreated(session) => self.apply_created_session(*session)?,
            ClientData::SessionsUpdated(sessions) => self.apply_sessions_updated(sessions),
            ClientData::History(history) => self.replace_history(history),
            ClientData::Plans(plans) => self.model.plans = plans,
            ClientData::Children(children) => self.model.children = children,
            ClientData::ChildHistory(history) => {
                self.model.attachment_state = crate::ui_model::SessionAttachmentState::Attached;
                self.model.turn_points = super::session_state::turn_points(&history);
                self.model.transcript = history.into_iter().map(TranscriptItem::History).collect();
            }
            ClientData::ModelSelected { provider, model } => {
                if let Some(session) = self.model.active_session.as_ref()
                    && let Some(summary) = self
                        .model
                        .sessions
                        .iter_mut()
                        .find(|summary| summary.session_id == session.as_str())
                {
                    summary.provider = Some(provider);
                    summary.model = Some(AgentRef {
                        id: model,
                        ..AgentRef::default()
                    });
                }
            }
            ClientData::QuestionAnswered => self.model.notice = None,
        }
        Ok(())
    }

    fn clear_host_error(&mut self, probe: &str) {
        self.model.host_errors.retain(|(name, _)| name != probe);
    }

    fn apply_client_error(&mut self, error: &ClientOperationError, destination: ErrorDestination) {
        let message = error.to_string();
        let recovered = message.contains("does not support effort")
            && self.model.reasoning != crate::ui_model::ReasoningLevel::Automatic
            && {
                self.model.reasoning = crate::ui_model::ReasoningLevel::Automatic;
                true
            };
        match destination {
            ErrorDestination::Transcript => {
                if recovered {
                    self.model.transcript.push(TranscriptItem::Status(
                        "Reasoning effort is not supported by this model; using auto.".to_owned(),
                    ));
                }
                self.model.transcript.push(TranscriptItem::Error(message));
            }
            ErrorDestination::Notice => self.model.notice = Some(message),
        }
        if recovered
            && matches!(destination, ErrorDestination::Transcript)
            && let Some(session) = self.model.active_session.clone()
        {
            self.client.set_reasoning(
                session,
                crate::ui_model::ReasoningLevel::Automatic,
                self.session_generation,
            );
        }
    }
}
