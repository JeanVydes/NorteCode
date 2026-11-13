use std::{future::Future, io, sync::Arc};

use futures_util::StreamExt;
use norte_code_client::{
    AgentClient, ApiError, ClientError, EventStream, RemoteClient, RestClient, SessionSummary,
};
use norte_code_wire::{
    ChildSessionSummary, ModelInfo, PlanSummary, QueueMode, ServerEvent, SessionKey,
};
use thiserror::Error;
use tokio::sync::mpsc::{self, Receiver, Sender};

mod catalog;
mod host;
mod imports;
mod jobs;
mod plugins;
mod reports;
mod turns;
mod uploads;

use crate::config::ClientConfig;

/// The queue policy the host spells for a mode.
fn queue_policy(mode: QueueMode) -> &'static str {
    match mode {
        QueueMode::Reject => "reject",
        QueueMode::Queue => "queue",
        QueueMode::Interrupt => "interrupt",
    }
}

#[derive(Debug, Error)]
pub(crate) enum ClientRuntimeError {
    #[error(transparent)]
    Runtime(#[from] io::Error),
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error(transparent)]
    Api(#[from] ApiError),
}

pub(crate) enum ClientEvent {
    Attached {
        session: SessionKey,
        generation: u64,
    },
    Server {
        session: SessionKey,
        generation: u64,
        event: Box<ServerEvent>,
    },
    Failure {
        session: SessionKey,
        generation: u64,
        message: String,
    },
    AttachmentFailure {
        session: SessionKey,
        generation: u64,
        message: String,
    },
    Imported {
        session: SessionKey,
        generation: u64,
        session_id: String,
    },
    Rewound {
        session: SessionKey,
        generation: u64,
        removed: Vec<String>,
    },
    Pinned {
        session: SessionKey,
        generation: u64,
        pinned_turns: Vec<String>,
    },
    Jobs {
        session: SessionKey,
        generation: u64,
        jobs: Vec<norte_code_wire::JobInfo>,
    },
    Uploaded {
        session: SessionKey,
        generation: u64,
        chip: crate::chips::Chip,
    },
    JobStopped {
        session: SessionKey,
        generation: u64,
        job: norte_code_wire::JobInfo,
    },
    Data {
        session: Option<SessionKey>,
        generation: u64,
        result: Box<Result<ClientData, ClientOperationError>>,
    },
}

#[derive(Debug, Error)]
pub(crate) enum ClientOperationError {
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error(transparent)]
    Api(#[from] ApiError),
}

pub(crate) enum ClientData {
    Models(Vec<ModelInfo>),
    Catalog(norte_code_client::CatalogDetail),
    HostFailure {
        probe: &'static str,
        message: String,
    },
    ServerInfo(norte_code_client::ServerInfo),
    Liveness(norte_code_client::Liveness),
    Readiness(norte_code_client::Readiness),
    Quota(norte_code_wire::QuotaReport),
    Agents(Vec<norte_code_wire::AgentSummary>),
    Skills(norte_code_client::SkillCatalog),
    Mcp(norte_code_client::McpCatalog),
    ToolRules(Vec<norte_code_client::ToolRule>),
    PluginUpdate {
        change: norte_code_client::PluginChange,
        plugins: Vec<norte_code_client::PluginSummary>,
    },
    Telemetry(norte_code_wire::TelemetryReport),
    Costs {
        session: Option<SessionKey>,
        report: norte_code_client::CostReport,
    },
    CompactedHistory(Vec<norte_code_wire::HistoryItem>),
    ExportReady(String),
    SessionCreated(Box<SessionSummary>),
    SessionsUpdated(Vec<SessionSummary>),
    History(Vec<norte_code_wire::HistoryItem>),
    Plans(Vec<PlanSummary>),
    Children(Vec<ChildSessionSummary>),
    ChildHistory(Vec<norte_code_wire::HistoryItem>),
    ModelSelected {
        provider: String,
        model: String,
    },
    QuestionAnswered,
}

pub(crate) struct ClientRuntime {
    runtime: tokio::runtime::Runtime,
    client: Arc<RemoteClient>,
    api: Arc<RestClient>,
    events: Receiver<ClientEvent>,
    sender: Sender<ClientEvent>,
    endpoint: String,
    authentication_configured: bool,
    agent_id: String,
    /// The task that reads the open stream of the active session.
    stream: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl ClientRuntime {
    pub(crate) fn connect(
        config: &ClientConfig,
    ) -> Result<(Self, Vec<SessionSummary>, SessionKey), ClientRuntimeError> {
        let base_url = config.service.url.clone();
        let token = config.service.token.as_deref();
        let agent_id = config.agent_id.clone();
        let runtime = tokio::runtime::Runtime::new()?;
        let client = Arc::new(
            RemoteClient::new(base_url.clone(), token, None)?.with_agent_id(agent_id.clone()),
        );
        let api = Arc::new(RestClient::new(&base_url, token.unwrap_or_default())?);
        let sessions = runtime.block_on(client.list_sessions())?;
        // The reader starts with an empty session of their own, never with one
        // that was left open before.
        let fresh = runtime.block_on(client.create_session(None))?;
        let (sender, events) = mpsc::channel(256);
        Ok((
            Self {
                runtime,
                client,
                api,
                events,
                sender,
                endpoint: base_url,
                authentication_configured: token.is_some(),
                agent_id,
                stream: std::sync::Mutex::new(None),
            },
            sessions,
            SessionKey::new(fresh.session_id),
        ))
    }

    pub(crate) fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub(crate) const fn authentication_configured(&self) -> bool {
        self.authentication_configured
    }

    /// Opens the session's event stream and keeps it open.
    ///
    /// The stream is the attachment: the host binds the session to it and
    /// releases it when the stream closes. Keeping it open means the reader
    /// sees every event of the session, whether a turn is running or not.
    pub(crate) fn resume(&self, session: SessionKey, generation: u64, agent_id: Option<String>) {
        if let Some(agent_id) = agent_id {
            self.client.set_session_agent(session.as_str(), agent_id);
        }
        self.close_stream();
        let request_session = session.clone();
        let client = Arc::clone(&self.client);
        let api = Arc::clone(&self.api);
        let sender = self.sender.clone();
        let handle = self.runtime.spawn(async move {
            match client.open_events(request_session.as_str()).await {
                Ok(mut stream) => {
                    if sender
                        .send(ClientEvent::Attached {
                            session: request_session.clone(),
                            generation,
                        })
                        .await
                        .is_err()
                    {
                        return;
                    }
                    let result = api
                        .history(request_session.as_str())
                        .await
                        .map(ClientData::History);
                    let _ = sender
                        .send(ClientEvent::Data {
                            session: Some(request_session.clone()),
                            generation,
                            result: Box::new(result.map_err(Into::into)),
                        })
                        .await;
                    while let Some(event) = stream.next().await {
                        if sender
                            .send(ClientEvent::Server {
                                session: request_session.clone(),
                                generation,
                                event: Box::new(event),
                            })
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                }
                Err(error) => {
                    let _ = sender
                        .send(ClientEvent::AttachmentFailure {
                            session,
                            generation,
                            message: error.to_string(),
                        })
                        .await;
                }
            }
        });
        self.remember_stream(handle);
    }

    /// Closes the stream of the session that was open.
    ///
    /// Dropping the stream releases the attachment on the host, so the next
    /// session starts from a clean state.
    fn close_stream(&self) {
        let handle = self
            .stream
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(handle) = handle {
            handle.abort();
        }
    }

    fn remember_stream(&self, handle: tokio::task::JoinHandle<()>) {
        let mut stream = self
            .stream
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(previous) = stream.replace(handle) {
            previous.abort();
        }
    }

    pub(crate) fn create_session(&self) {
        let api = Arc::clone(&self.api);
        let agent_id = self.agent_id.clone();
        self.spawn_data(None, 0, async move {
            api.create_session(&agent_id, None, None)
                .await
                .map(|session| ClientData::SessionCreated(Box::new(session)))
        });
    }

    pub(crate) fn rename_session(&self, session: SessionKey, title: String, generation: u64) {
        let client = Arc::clone(&self.client);
        self.spawn_client_data(None, generation, async move {
            client.rename_session(session.as_str(), &title).await?;
            client
                .list_sessions()
                .await
                .map(ClientData::SessionsUpdated)
        });
    }

    pub(crate) fn delete_session(&self, session: SessionKey, generation: u64) {
        let client = Arc::clone(&self.client);
        self.spawn_client_data(None, generation, async move {
            client.delete_session(session.as_str()).await?;
            let sessions = client.list_sessions().await?;
            if sessions.is_empty() {
                return client
                    .create_session(None)
                    .await
                    .map(|session| ClientData::SessionCreated(Box::new(session)));
            }
            Ok(ClientData::SessionsUpdated(sessions))
        });
    }

    pub(crate) fn fork_session(&self, session: SessionKey, generation: u64) {
        let client = Arc::clone(&self.client);
        self.spawn_client_data(None, generation, async move {
            client
                .fork_session(session.as_str())
                .await
                .map(|session| ClientData::SessionCreated(Box::new(session)))
        });
    }

    pub(crate) fn compact_session(&self, session: SessionKey, generation: u64) {
        let client = Arc::clone(&self.client);
        let api = Arc::clone(&self.api);
        let sender = self.sender.clone();
        self.runtime.spawn(async move {
            let mut stream = match client.compact_session(session.as_str()).await {
                Ok(stream) => stream,
                Err(error) => {
                    let _ = sender
                        .send(ClientEvent::Data {
                            session: Some(session),
                            generation,
                            result: Box::new(Err(error.into())),
                        })
                        .await;
                    return;
                }
            };
            while let Some(event) = stream.next().await {
                if sender
                    .send(ClientEvent::Server {
                        session: session.clone(),
                        generation,
                        event: Box::new(event),
                    })
                    .await
                    .is_err()
                {
                    return;
                }
            }
            let result = api
                .history(session.as_str())
                .await
                .map(ClientData::CompactedHistory)
                .map_err(Into::into);
            let _ = sender
                .send(ClientEvent::Data {
                    session: Some(session),
                    generation,
                    result: Box::new(result),
                })
                .await;
        });
    }

    pub(crate) fn export_session(&self, session: SessionKey, generation: u64) {
        let api = Arc::clone(&self.api);
        self.spawn_data(Some(session.clone()), generation, async move {
            let export_id = api.export_session(session.as_str()).await?;
            Ok(ClientData::ExportReady(api.export_url(&export_id)))
        });
    }

    pub(crate) fn set_budget(
        &self,
        session: SessionKey,
        limit: f64,
        action: norte_code_wire::BudgetAction,
        generation: u64,
    ) {
        let client = Arc::clone(&self.client);
        self.spawn_stream(session.clone(), generation, async move {
            client
                .set_budget(
                    session.as_str(),
                    norte_code_wire::Budget {
                        unit: norte_code_wire::BudgetUnit::Usd,
                        limit,
                        action,
                    },
                )
                .await
        });
    }

    pub(crate) fn approve_budget(&self, session: SessionKey, limit: f64, generation: u64) {
        let client = Arc::clone(&self.client);
        let sender = self.sender.clone();
        self.runtime.spawn(async move {
            let continuation =
                Self::continue_after_budget_approval(&client, &sender, &session, limit, generation)
                    .await;
            let restoration = Self::set_budget_action(
                &client,
                &sender,
                &session,
                limit,
                norte_code_wire::BudgetAction::Ask,
                generation,
            )
            .await;
            if let Err(error) = continuation {
                Self::send_operation_error(&sender, session.clone(), generation, error).await;
            }
            if let Err(error) = restoration {
                Self::send_operation_error(&sender, session, generation, error).await;
            }
        });
    }

    /// Sends a message to the session.
    ///
    /// The message goes to the command endpoint and the events of the turn
    /// arrive on the session's open stream, so the reader sees them the same
    /// way it sees every other event.
    pub(crate) fn send_message(
        &self,
        session: SessionKey,
        generation: u64,
        text: String,
        parts: Vec<norte_code_wire::UserInputPart>,
        queue_mode: QueueMode,
    ) {
        let api = Arc::clone(&self.api);
        let sender = self.sender.clone();
        self.runtime.spawn(async move {
            let policy = queue_policy(queue_mode);
            let outcome = if parts.is_empty() {
                api.send_prompt(session.as_str(), Some(text.as_str()), None, None, policy)
                    .await
            } else {
                let mut content = parts;
                if !text.is_empty() {
                    content.insert(0, norte_code_wire::UserInputPart::text(text));
                }
                api.send_prompt(session.as_str(), None, Some(content), None, policy)
                    .await
            };
            if let Err(error) = outcome {
                let _ = sender
                    .send(ClientEvent::Failure {
                        session,
                        generation,
                        message: error.to_string(),
                    })
                    .await;
            }
        });
    }

    pub(crate) fn load_models(&self, generation: u64) {
        let api = Arc::clone(&self.api);
        self.spawn_data(None, generation, async move {
            api.models().await.map(ClientData::Models)
        });
    }

    pub(crate) fn load_catalog(&self, generation: u64) {
        let api = Arc::clone(&self.api);
        self.spawn_data(None, generation, async move {
            api.catalog_detail().await.map(ClientData::Catalog)
        });
    }

    pub(crate) fn refresh_sessions(&self, generation: u64) {
        let client = Arc::clone(&self.client);
        self.spawn_client_data(None, generation, async move {
            client
                .list_sessions()
                .await
                .map(ClientData::SessionsUpdated)
        });
    }

    pub(crate) fn load_plans(&self, session: SessionKey, generation: u64) {
        let api = Arc::clone(&self.api);
        let request_session = session.clone();
        self.spawn_data(Some(session), generation, async move {
            api.plans(request_session.as_str())
                .await
                .map(ClientData::Plans)
        });
    }

    pub(crate) fn load_children(&self, session: SessionKey, generation: u64) {
        let api = Arc::clone(&self.api);
        let request_session = session.clone();
        self.spawn_data(Some(session), generation, async move {
            api.child_sessions(request_session.as_str())
                .await
                .map(ClientData::Children)
        });
    }

    pub(crate) fn load_child_history(
        &self,
        session: SessionKey,
        agent: Option<String>,
        generation: u64,
    ) {
        let api = Arc::clone(&self.api);
        let request_session = session.clone();
        let agent_id = agent.unwrap_or_else(|| self.agent_id.clone());
        let sender = self.sender.clone();
        self.runtime.spawn(async move {
            let result = async {
                api.attach_observer(request_session.as_str(), &agent_id)
                    .await?;
                api.history(request_session.as_str()).await
            }
            .await;
            match result {
                Ok(history) => {
                    let _ = sender
                        .send(ClientEvent::Data {
                            session: Some(session),
                            generation,
                            result: Box::new(Ok(ClientData::ChildHistory(history))),
                        })
                        .await;
                }
                Err(error) => {
                    let _ = sender
                        .send(ClientEvent::AttachmentFailure {
                            session,
                            generation,
                            message: error.to_string(),
                        })
                        .await;
                }
            }
        });
    }

    pub(crate) fn set_model(
        &self,
        session: SessionKey,
        generation: u64,
        provider: String,
        model: String,
    ) {
        let api = Arc::clone(&self.api);
        let request_session = session.clone();
        let selected_provider = provider.clone();
        let selected_model = model.clone();
        self.spawn_data(Some(session), generation, async move {
            api.set_session_model(request_session.as_str(), &provider, &model)
                .await
                .map(|()| ClientData::ModelSelected {
                    provider: selected_provider,
                    model: selected_model,
                })
        });
    }

    pub(crate) fn answer_question(
        &self,
        session: SessionKey,
        generation: u64,
        question: String,
        answer: String,
    ) {
        let api = Arc::clone(&self.api);
        let request_session = session.clone();
        self.spawn_data(Some(session), generation, async move {
            api.answer_question(request_session.as_str(), &question, &answer)
                .await
                .map(|()| ClientData::QuestionAnswered)
        });
    }

    pub(crate) fn try_recv(&mut self) -> Option<ClientEvent> {
        self.events.try_recv().ok()
    }

    fn spawn_stream<F>(&self, session: SessionKey, generation: u64, request: F)
    where
        F: Future<Output = Result<EventStream, ClientError>> + Send + 'static,
    {
        let sender = self.sender.clone();
        self.runtime.spawn(async move {
            match request.await {
                Ok(mut stream) => {
                    while let Some(event) = stream.next().await {
                        if sender
                            .send(ClientEvent::Server {
                                session: session.clone(),
                                generation,
                                event: Box::new(event),
                            })
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                }
                Err(error) => {
                    let _ = sender
                        .send(ClientEvent::Failure {
                            session,
                            generation,
                            message: error.to_string(),
                        })
                        .await;
                }
            }
        });
    }

    async fn forward_events(
        mut stream: EventStream,
        sender: &Sender<ClientEvent>,
        session: &SessionKey,
        generation: u64,
    ) -> bool {
        while let Some(event) = stream.next().await {
            if sender
                .send(ClientEvent::Server {
                    session: session.clone(),
                    generation,
                    event: Box::new(event),
                })
                .await
                .is_err()
            {
                return false;
            }
        }
        true
    }

    async fn continue_after_budget_approval(
        client: &RemoteClient,
        sender: &Sender<ClientEvent>,
        session: &SessionKey,
        limit: f64,
        generation: u64,
    ) -> Result<bool, ClientError> {
        let changed = Self::set_budget_action(
            client,
            sender,
            session,
            limit,
            norte_code_wire::BudgetAction::Warn,
            generation,
        )
        .await?;
        let stream = client.resume(session.as_str()).await?;
        let resumed = Self::forward_events(stream, sender, session, generation).await;
        Ok(changed && resumed)
    }

    async fn set_budget_action(
        client: &RemoteClient,
        sender: &Sender<ClientEvent>,
        session: &SessionKey,
        limit: f64,
        action: norte_code_wire::BudgetAction,
        generation: u64,
    ) -> Result<bool, ClientError> {
        let stream = client
            .set_budget(
                session.as_str(),
                norte_code_wire::Budget {
                    unit: norte_code_wire::BudgetUnit::Usd,
                    limit,
                    action,
                },
            )
            .await?;
        Ok(Self::forward_events(stream, sender, session, generation).await)
    }

    async fn send_operation_error(
        sender: &Sender<ClientEvent>,
        session: SessionKey,
        generation: u64,
        error: ClientError,
    ) {
        let _ = sender
            .send(ClientEvent::Failure {
                session,
                generation,
                message: error.to_string(),
            })
            .await;
    }

    fn spawn_data<F>(&self, session: Option<SessionKey>, generation: u64, request: F)
    where
        F: Future<Output = Result<ClientData, ApiError>> + Send + 'static,
    {
        let sender = self.sender.clone();
        self.runtime.spawn(async move {
            let _ = sender
                .send(ClientEvent::Data {
                    session,
                    generation,
                    result: Box::new(request.await.map_err(Into::into)),
                })
                .await;
        });
    }

    fn spawn_client_data<F>(&self, session: Option<SessionKey>, generation: u64, request: F)
    where
        F: Future<Output = Result<ClientData, ClientError>> + Send + 'static,
    {
        let sender = self.sender.clone();
        self.runtime.spawn(async move {
            let _ = sender
                .send(ClientEvent::Data {
                    session,
                    generation,
                    result: Box::new(request.await.map_err(Into::into)),
                })
                .await;
        });
    }
}
