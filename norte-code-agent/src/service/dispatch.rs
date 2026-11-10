//! One of the service's domains, as `impl Service` blocks.
//!
//! The parent's imports are in scope through `super`, so the domain
//! names types the same way the service does.

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::*;

impl Service {
    /// Replaces every session name in a request with the session it names.
    ///
    /// The only place a short name becomes a full one. It is a match over the
    /// request rather than a method on it so the protocol crate stays free of
    /// the service's session table, and every arm that carries a
    /// [`SessionKey`] is listed: one that is missed would keep answering
    /// "unknown" for a name the footer shows.
    #[allow(clippy::too_many_lines)] // one arm per request variant, which is the enum.
    pub(crate) fn canonical(
        &self,
        kind: norte_code_wire::ClientRequestKind,
    ) -> Result<norte_code_wire::ClientRequestKind, ServiceError> {
        use norte_code_wire::ClientRequestKind as Kind;
        let resolve = |key: SessionKey| self.resolve_key(&key);
        Ok(match kind {
            Kind::ForkSession {
                session,
                title,
                from_turn_id,
                inclusive,
                carry_usage,
                agent_id,
                include,
            } => Kind::ForkSession {
                session: resolve(session)?,
                title,
                from_turn_id,
                inclusive,
                carry_usage,
                agent_id,
                include,
            },
            Kind::RenameSession { session, title } => Kind::RenameSession {
                session: resolve(session)?,
                title,
            },
            Kind::DeleteSession { session } => Kind::DeleteSession {
                session: resolve(session)?,
            },
            Kind::SendMessage {
                session,
                text,
                attachments,
                budget,
                reasoning,
                output,
                response_format,
                metadata,
                replay,
            } => Kind::SendMessage {
                session: session.map(resolve).transpose()?,
                text,
                attachments,
                budget,
                reasoning,
                output,
                response_format,
                metadata,
                replay,
            },
            Kind::Steer {
                session,
                text,
                id,
                queue_mode,
            } => Kind::Steer {
                session: resolve(session)?,
                text,
                id,
                queue_mode,
            },
            Kind::CancelTurn { session } => Kind::CancelTurn {
                session: resolve(session)?,
            },
            Kind::StopJobs { session, id } => Kind::StopJobs {
                session: resolve(session)?,
                id,
            },
            Kind::DetachTurn { session } => Kind::DetachTurn {
                session: resolve(session)?,
            },
            Kind::ListJobs { session } => Kind::ListJobs {
                session: resolve(session)?,
            },
            Kind::Resume { session, replay } => Kind::Resume {
                session: resolve(session)?,
                replay,
            },
            Kind::AnswerQuestion {
                session,
                question,
                answer,
            } => Kind::AnswerQuestion {
                session: resolve(session)?,
                question,
                answer,
            },
            Kind::SetModel {
                session,
                provider,
                model,
            } => Kind::SetModel {
                session: resolve(session)?,
                provider,
                model,
            },
            Kind::SetReasoning {
                session,
                scope,
                reasoning,
            } => Kind::SetReasoning {
                session: resolve(session)?,
                scope,
                reasoning,
            },
            Kind::SetBudget { session, budget } => Kind::SetBudget {
                session: resolve(session)?,
                budget,
            },
            Kind::Compact { session } => Kind::Compact {
                session: resolve(session)?,
            },
            Kind::SessionCompact {
                session,
                strategy,
                keep_last_turns,
                instructions,
                trigger,
            } => Kind::SessionCompact {
                session: resolve(session)?,
                strategy,
                keep_last_turns,
                instructions,
                trigger,
            },
            Kind::ContextPin {
                session,
                turn_ids,
                pinned,
            } => Kind::ContextPin {
                session: resolve(session)?,
                turn_ids,
                pinned,
            },
            Kind::SessionRewind {
                session,
                to_turn_id,
                inclusive,
                cancel_pending,
                workspace,
            } => Kind::SessionRewind {
                session: resolve(session)?,
                to_turn_id,
                inclusive,
                cancel_pending,
                workspace,
            },
            Kind::HistoryGet {
                session,
                before_turn_id,
                limit,
                include_rewound,
            } => Kind::HistoryGet {
                session: resolve(session)?,
                before_turn_id,
                limit,
                include_rewound,
            },
            Kind::CostReport { session } => Kind::CostReport {
                session: session.map(resolve).transpose()?,
            },
            // Every other request names no session, so there is nothing to
            // resolve and it is returned as it arrived.
            other => other,
        })
    }
    /// Answers one client request.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] when the request cannot be started; a failure
    /// that happens while a turn runs is delivered as a
    /// [`ServerEventKind::Error`] event instead.
    #[allow(clippy::too_many_lines)]
    pub async fn submit(
        self: &Arc<Self>,
        mut request: norte_code_wire::ClientRequest,
    ) -> Result<EventStream, ServiceError> {
        use norte_code_wire::ClientRequestKind as Kind;
        // A session may be named by a prefix of its id: the footer shows eight
        // characters and a person types back what they can see. Resolve it once
        // here, at the one door every request comes through, so no handler has
        // to know a name may be short: it is handed the canonical key or the
        // error that says why it is not one.
        request.kind = self.canonical(request.kind)?;
        let request_id = request.request_id;
        match request.kind {
            Kind::CreateSession { title } => {
                let key = self.create_session(title.clone()).await?;
                Ok(single(
                    request_id,
                    Some(key),
                    ServerEventKind::SessionCreated { title },
                ))
            }
            Kind::ListSessions => Ok(single(
                request_id,
                None,
                ServerEventKind::SessionList {
                    sessions: self.list_live_sessions().await,
                },
            )),
            Kind::ForkSession {
                session,
                title,
                from_turn_id,
                inclusive,
                carry_usage,
                agent_id: _,
                include: _,
            } => {
                let (key, from_turn_id) = self
                    .fork_session(
                        &session,
                        title.clone(),
                        from_turn_id.as_deref(),
                        inclusive.unwrap_or(true),
                        carry_usage.unwrap_or(true),
                    )
                    .await?;
                Ok(single(
                    request_id,
                    Some(key),
                    ServerEventKind::SessionForked {
                        from: session,
                        from_turn_id,
                        title,
                    },
                ))
            }
            Kind::RenameSession { session, title } => {
                self.rename(&session, &title).await?;
                Ok(single(
                    request_id,
                    Some(session),
                    ServerEventKind::SessionRenamed { title },
                ))
            }
            Kind::DeleteSession { session } => {
                self.delete(&session).await?;
                Ok(single(
                    request_id,
                    Some(session),
                    ServerEventKind::SessionDeleted,
                ))
            }
            Kind::SendMessage {
                session,
                text,
                attachments,
                budget,
                reasoning,
                output: _,
                response_format,
                metadata: _,
                replay,
            } => {
                // `response_format` (RFC 12.6) is validated here, not only in
                // the host: the service is the layer that has the schema,
                // the correction message, and the `OUTPUT_SCHEMA_INVALID`
                // terminal. The host validates the shape and the
                // `structured_output` capability before the turn starts.
                if let Some(format) = &response_format
                    && format.kind == norte_code_wire::ResponseFormatType::JsonSchema
                {
                    let Some(schema) = &format.schema else {
                        return Err(ServiceError::Unsupported(
                            "response_format json_schema requires schema",
                        ));
                    };
                    let bytes = serde_json::to_vec(schema).unwrap_or_default().len();
                    if bytes > 16 * 1024 {
                        return Err(ServiceError::Unsupported(
                            "response_format schema exceeds 16 KiB",
                        ));
                    }
                }
                self.start_turn(
                    request_id,
                    session,
                    Some(text),
                    attachments,
                    Some(budget),
                    reasoning,
                    replay,
                )
                .await
            }
            Kind::Resume { session, replay } => {
                self.plans
                    .ensure_session_folder(&session)
                    .await
                    .map_err(ServiceError::Plan)?;
                // A resume exists to show where the conversation was.
                self.start_turn(
                    request_id,
                    Some(session),
                    None,
                    Vec::new(),
                    None,
                    None,
                    replay,
                )
                .await
            }
            Kind::CancelTurn { session } => {
                self.cancel_turn_and_watch(&session);
                Ok(Box::pin(futures_util::stream::empty()))
            }
            Kind::StopJobs { session, id } => {
                // A job's token is its own: stopping one is not cancelling the
                // turn, and this is the path that says so.
                let stopped = {
                    let sessions = self.lock_sessions();
                    match sessions.get(&session) {
                        Some(record) => match id.as_deref() {
                            Some(id) => usize::from(self.jobs.cancel_in_session(record.id, id)),
                            None => self.jobs.cancel_session(record.id),
                        },
                        None => 0,
                    }
                };
                Ok(Box::pin(futures_util::stream::iter([ServerEvent::new(
                    request_id,
                    Some(session),
                    ServerEventKind::JobsStopped { stopped },
                )])))
            }
            Kind::DetachTurn { session } => {
                // Detaching is not cancelling: the call stops waiting and the
                // process keeps running. The token is the one the executor
                // published for the session, so no turn state is touched here.
                let detached = self
                    .lock_sessions()
                    .get(&session)
                    .is_some_and(|record| self.detaches.detach(record.id));
                Ok(Box::pin(futures_util::stream::iter([ServerEvent::new(
                    request_id,
                    Some(session),
                    ServerEventKind::TurnStopped {
                        reason: if detached {
                            "the running call was detached; its process keeps going".to_owned()
                        } else {
                            "there was no running call to detach".to_owned()
                        },
                    },
                )])))
            }
            Kind::ListJobs { session } => {
                let jobs = match self.lock_sessions().get(&session) {
                    Some(record) => self.jobs.list(Some(record.id)),
                    None => Vec::new(),
                };
                Ok(single(
                    request_id,
                    Some(session),
                    ServerEventKind::JobList {
                        jobs: jobs.iter().map(job_info).collect(),
                    },
                ))
            }
            Kind::CostReport { session } => Ok(self.cost_report(request_id, session.as_ref())),
            Kind::ListTelemetry { session } => {
                self.telemetry_report(request_id, session.as_ref()).await
            }
            Kind::ListTools { session } => Ok(self.tool_list_event(request_id, session.as_ref())),
            Kind::ListMcp => Ok(self.mcp_list_event(request_id)),
            Kind::RefreshMcp => {
                self.reload_mcp().await;
                Ok(self.mcp_list_event(request_id))
            }
            Kind::SetMcpActive { name, active } => {
                if self.set_mcp_active(&name, active).is_none() {
                    return Err(ServiceError::Unsupported("unknown MCP server"));
                }
                Ok(self.mcp_list_event(request_id))
            }
            Kind::SetApprovalRule {
                session,
                tool,
                allow,
                global,
            } => {
                self.set_approval_rule(session.as_ref(), &tool, allow, global)
                    .await?;
                Ok(self.tool_list_event(request_id, session.as_ref()))
            }
            Kind::ForgetApprovalRule {
                session,
                tool,
                global,
            } => {
                self.forget_approval_rule(session.as_ref(), &tool, global)
                    .await?;
                Ok(self.tool_list_event(request_id, session.as_ref()))
            }
            Kind::ListModels => Ok(self.list_models_event(request_id)),
            Kind::ListCatalog => Ok(self.catalog_detail_event(request_id)),
            Kind::ListSkills => Ok(self.skill_list_event(request_id)),
            Kind::ListPlans { session } => self.list_plans_event(request_id, session).await,
            Kind::RefreshSkills => Ok(self.refresh_skills_event(request_id)),
            Kind::RefreshCatalog => self.reload_catalog(request_id),
            Kind::CatalogUpsertProvider {
                provider_id,
                provider,
            } => self.upsert_provider(request_id, &provider_id, provider),
            Kind::CatalogRemoveProvider { provider_id } => {
                self.remove_provider(request_id, &provider_id)
            }
            Kind::CatalogUpsertModel { model } => self.upsert_model(request_id, model),
            Kind::CatalogRemoveModel {
                provider_id,
                model_id,
            } => self.remove_model(request_id, &provider_id, &model_id),
            Kind::SetModel {
                session,
                provider,
                model,
            } => {
                self.set_model_event(request_id, session, provider, model)
                    .await
            }
            Kind::SetBudget { session, budget } => {
                self.set_budget(request_id, session, budget).await
            }
            Kind::SetReasoning {
                session,
                scope,
                reasoning,
            } => {
                self.set_reasoning(request_id, session, scope, reasoning)
                    .await
            }
            Kind::AnswerQuestion {
                session,
                question,
                answer,
            } => {
                self.answer_question(request_id, session, question, answer)
                    .await
            }
            Kind::Steer {
                session,
                text,
                id,
                queue_mode,
            } => self.steer(request_id, session, text, id, queue_mode).await,
            Kind::Compact { session } => self.compact_event_stream(request_id, session).await,
            Kind::SessionCompact { session, .. } => {
                self.compact_event_stream(request_id, session).await
            }
            Kind::ContextPin {
                session,
                turn_ids: _,
                pinned: _,
            } => self.context_pin_event(request_id, session).await,
            Kind::SessionRewind { session, .. } => self.rewind_event(request_id, session).await,
            Kind::HistoryGet { session, .. } => self.history_get_event(request_id, session).await,
            Kind::SessionExport {
                session,
                include,
                quiesce,
            } => {
                self.export_session(request_id, &session, include, quiesce)
                    .await
            }
            Kind::SessionImport {
                bundle,
                decryption_jwk,
            } => {
                self.import_session(request_id, bundle, decryption_jwk)
                    .await
            }
            Kind::ListPlugins
            | Kind::InstallPlugin { .. }
            | Kind::UpdatePlugins { .. }
            | Kind::RemovePlugin { .. }
            | Kind::SetPluginEnabled { .. } => Ok(single(
                request_id,
                None,
                ServerEventKind::Error {
                    code: norte_code_wire::ErrorCode::InvalidRequest,
                    message: "plugin lifecycle is managed by the RAEP host".to_owned(),
                },
            )),
        }
    }
    /// The budget decision that stopped the turn, if the layer made one.
    pub(crate) async fn telemetry_report(
        &self,
        request_id: u64,
        session: Option<&SessionKey>,
    ) -> Result<EventStream, ServiceError> {
        let records = self
            .directory
            .requests(session.map(ToString::to_string).as_deref())
            .await
            .map_err(|error| ServiceError::Telemetry(error.to_string()))?;
        Ok(single(
            request_id,
            session.cloned(),
            ServerEventKind::Telemetry {
                records: records.iter().map(Into::into).collect(),
            },
        ))
    }
    pub(crate) fn tool_list_event(
        &self,
        request_id: u64,
        session: Option<&SessionKey>,
    ) -> EventStream {
        let tools = self
            .tool_rules(session)
            .into_iter()
            .map(|tool| norte_code_wire::ToolInfo {
                name: tool.name,
                allowed: tool.allowed,
                scope: match tool.scope {
                    RuleScope::Session => "session",
                    RuleScope::Global => "global",
                    RuleScope::Default => "default",
                }
                .to_owned(),
            })
            .collect();
        single(
            request_id,
            session.cloned(),
            ServerEventKind::ToolList { tools },
        )
    }
    pub(crate) fn mcp_list_event(&self, request_id: u64) -> EventStream {
        let servers = self
            .mcp_status()
            .into_iter()
            .map(|server| norte_code_wire::McpInfo {
                name: server.name,
                command: server.command,
                tools: server.tools,
                filtered: server.filtered,
                active: server.active,
            })
            .collect();
        let failures = self
            .mcp_failures()
            .into_iter()
            .map(|(name, reason)| norte_code_wire::McpFailure { name, reason })
            .collect();
        single(
            request_id,
            None,
            ServerEventKind::McpList { servers, failures },
        )
    }
    pub(crate) fn cost_report(&self, request_id: u64, session: Option<&SessionKey>) -> EventStream {
        let lock = self.lock_sessions();
        let (cost, requests, unpriced) = match session {
            Some(key) => lock.get(key).map_or((0.0, 0, 0), |record| {
                (record.cost, record.requests, record.unpriced)
            }),
            None => lock
                .values()
                .fold((0.0, 0, 0), |(cost, requests, unpriced), record| {
                    (
                        cost + record.cost,
                        requests + record.requests,
                        unpriced + record.unpriced,
                    )
                }),
        };
        drop(lock);
        let mut events = vec![ServerEvent::new(
            request_id,
            session.cloned(),
            ServerEventKind::CostUpdated {
                total_usd: cost,
                requests,
            },
        )];
        if unpriced > 0 {
            events.push(ServerEvent::new(
                request_id,
                session.cloned(),
                ServerEventKind::CostNotReported { requests: unpriced },
            ));
        }
        Box::pin(futures_util::stream::iter(events))
    }
    /// Validates and sets the session's budget.
    ///
    /// A USD budget over a provider that never reported cost cannot be
    /// measured, so it is refused instead of silently comparing against zero.
    pub(crate) async fn steer(
        &self,
        _request_id: u64,
        key: SessionKey,
        text: String,
        id: String,
        queue_mode: norte_code_wire::QueueMode,
    ) -> Result<EventStream, ServiceError> {
        // The message takes the request already in flight. The durable copy
        // stays in the session until the engine reports that it applied it, so
        // a crash cannot lose it.
        let (nid, _, _) = self.require_record(&key)?;
        let busy = self.turn_lock(&key).try_lock().is_err();
        match queue_mode {
            norte_code_wire::QueueMode::Reject if busy => {
                return Err(ServiceError::SessionBusy);
            }
            norte_code_wire::QueueMode::Interrupt if busy => {
                self.cancel_turn(&key);
            }
            norte_code_wire::QueueMode::Reject
            | norte_code_wire::QueueMode::Queue
            | norte_code_wire::QueueMode::Interrupt => {}
        }
        let id = if id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            id
        };
        let steered = Steered::new(id.clone(), text);
        {
            let mut sessions = self.lock_sessions();
            let record = sessions
                .get_mut(&key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            if record.pending_steers.len() >= self.max_queued_inputs {
                return Err(ServiceError::SessionBusy);
            }
            record.pending_steers.push(steered.clone());
        }
        self.persist(&key).await?;
        self.steering.push(nid, steered);
        Ok(Box::pin(futures_util::stream::empty()))
    }
    pub(crate) async fn queue_job_result(
        &self,
        session: norte::SessionId,
        steered: &steering::Steered,
    ) -> Result<(), ServiceError> {
        let key = self
            .lock_sessions()
            .iter()
            .find(|(_, record)| record.id == session)
            .map(|(key, _)| key.clone())
            .ok_or_else(|| ServiceError::UnknownSession(session.to_string()));
        let key = match key {
            Ok(key) => key,
            Err(error) => return Err(error),
        };
        let queued = {
            let mut sessions = self.lock_sessions();
            let record = sessions
                .get_mut(&key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            if record.pending_steers.len() >= self.max_queued_inputs {
                false
            } else {
                record.pending_steers.push(steered.clone());
                true
            }
        };
        if !queued {
            return Err(ServiceError::SessionBusy);
        }
        self.persist(&key).await
    }
}
