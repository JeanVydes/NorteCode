//! Catalogue projection and reasoning resolution for a selected model.
//!
//! It also owns the switch between models: a switch that arrives while the
//! session is busy is kept until the next turn boundary, where it is applied
//! or dropped.

use std::sync::Arc;

use norte::{
    Message, Reasoning as ReasoningCapability, ReasoningEffort, ReasoningRequest, ToolKind,
};
use norte_code_catalog::{Catalog, LoadOptions, load_catalog};
use norte_code_config::Config;
use norte_code_protocol::{
    ModelInfo, Reasoning as ProtocolReasoning, ReasoningInfo, ServerEvent, ServerEventKind,
    SessionKey,
};

use crate::ServiceError;
use crate::provider::{ModelSpec, Wire};

use super::mapping::single;
use super::{EventStream, Service};

/// Flattens a catalogue for a picker, applying the config overrides.
#[must_use]
pub fn model_list(config: &Config, catalog: &Catalog) -> Vec<ModelInfo> {
    let mut models = Vec::new();
    for entry in catalog.models() {
        let wire = Wire::from_npm(entry.provider.npm.as_deref().unwrap_or_default());
        let overrides = config
            .model_overrides
            .get(&format!("{}/{}", entry.provider.id, entry.model.id));
        let capabilities = norte_code_catalog::capabilities(entry.model, wire, overrides);
        let mut efforts: Vec<String> = capabilities
            .reasoning
            .efforts
            .iter()
            .map(|effort| effort_name(*effort).to_owned())
            .collect();
        efforts.sort();
        models.push(ModelInfo {
            provider: entry.provider.id.clone(),
            model: entry.model.id.clone(),
            name: entry.model.name.clone(),
            description: entry.model.description.clone(),
            max_context: capabilities.max_context,
            max_output: capabilities.max_output,
            tools: capabilities.tool_kinds.contains(&ToolKind::Function),
            input: entry.model.modalities.input.clone(),
            output: entry.model.modalities.output.clone(),
            reasoning: ReasoningInfo {
                supported: capabilities.reasoning.supported,
                can_disable: capabilities.reasoning.can_disable,
                efforts,
                supports_budget: capabilities.reasoning.supports_budget,
                max_budget: capabilities.reasoning.max_budget,
            },
            deprecated: entry.model.status.as_deref() == Some("deprecated"),
        });
    }
    models
}

/// Resolves the effective reasoning setting without turning it into a Norte
/// request.
///
/// An explicit setting wins over the configured default. Every setting is
/// checked before the turn starts: an unsupported effort or budget is a typed
/// error, never a silent downgrade. This is also what debug output shows: an
/// implicit `auto` becomes medium (or the closest lower declared effort) when
/// the model supports reasoning.
///
/// # Errors
///
/// Returns [`ServiceError::Reasoning`] when the setting is malformed, the
/// model does not support it, or a budget exceeds the model's cap.
pub fn effective_reasoning(
    requested: Option<&ProtocolReasoning>,
    configured: &serde_json::Value,
    capabilities: &ReasoningCapability,
) -> Result<ProtocolReasoning, ServiceError> {
    let setting = if let Some(setting) = requested {
        setting.clone()
    } else {
        let configured = parse_configured_reasoning(configured).map_err(ServiceError::Reasoning)?;
        // The implicit default is medium: it leaves room for a useful answer
        // without asking every model for its most expensive effort. When a
        // model does not declare medium, prefer the closest effort below it
        // rather than jumping to high. Models with no declared efforts keep
        // `auto`, which is the only safe request.
        if matches!(configured, ProtocolReasoning::Automatic) {
            implicit_effort(capabilities).map_or(configured, |effort| ProtocolReasoning::Effort {
                effort: effort_name(effort).to_owned(),
            })
        } else {
            configured
        }
    };
    validate_reasoning(&setting, capabilities)?;
    Ok(setting)
}

/// Resolves the effective setting and maps it to the engine's request type.
pub(super) fn resolve_reasoning(
    requested: Option<&ProtocolReasoning>,
    configured: &serde_json::Value,
    capabilities: &ReasoningCapability,
) -> Result<ReasoningRequest, ServiceError> {
    let setting = effective_reasoning(requested, configured, capabilities)?;
    validate_reasoning(&setting, capabilities)
}

/// The effort an implicit `auto` resolves to.
///
/// Medium is the default. When the model does not declare it, the closest
/// lower effort avoids the top end; high is used only when it is the sole
/// declared effort.
fn implicit_effort(capabilities: &ReasoningCapability) -> Option<ReasoningEffort> {
    if !capabilities.supported {
        return None;
    }
    [
        ReasoningEffort::Medium,
        ReasoningEffort::Low,
        ReasoningEffort::Minimal,
        ReasoningEffort::High,
    ]
    .into_iter()
    .find(|effort| capabilities.efforts.contains(effort))
}

/// Parses `reasoning.default` from the config.
pub(super) fn parse_configured_reasoning(
    value: &serde_json::Value,
) -> Result<ProtocolReasoning, String> {
    match value {
        serde_json::Value::Null => Ok(ProtocolReasoning::Automatic),
        serde_json::Value::String(text) => parse_reasoning_name(text),
        serde_json::Value::Number(number) => {
            let tokens = number
                .as_u64()
                .and_then(|value| u32::try_from(value).ok())
                .filter(|value| *value > 0)
                .ok_or_else(|| "a reasoning budget must be a positive integer".to_owned())?;
            Ok(ProtocolReasoning::Budget { tokens })
        }
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(kind)) = map.get("reasoning") {
                match kind.as_str() {
                    "automatic" | "auto" => return Ok(ProtocolReasoning::Automatic),
                    "disabled" | "off" => return Ok(ProtocolReasoning::Disabled),
                    "effort" => {
                        let effort = map
                            .get("effort")
                            .and_then(serde_json::Value::as_str)
                            .ok_or_else(|| "`reasoning.effort` must be a string".to_owned())?;
                        return parse_reasoning_name(effort);
                    }
                    "budget" => {
                        let tokens = map
                            .get("tokens")
                            .and_then(serde_json::Value::as_u64)
                            .and_then(|value| u32::try_from(value).ok())
                            .filter(|value| *value > 0)
                            .ok_or_else(|| {
                                "`reasoning.tokens` must be a positive integer".to_owned()
                            })?;
                        return Ok(ProtocolReasoning::Budget { tokens });
                    }
                    other => {
                        return Err(format!(
                            "unknown reasoning setting `{other}`; use `auto`, `off`, an effort or a budget"
                        ));
                    }
                }
            }
            if let Some(tokens) = map.get("budget").or_else(|| map.get("tokens")) {
                let tokens = tokens
                    .as_u64()
                    .and_then(|value| u32::try_from(value).ok())
                    .filter(|value| *value > 0)
                    .ok_or_else(|| "`reasoning.budget` must be a positive integer".to_owned())?;
                return Ok(ProtocolReasoning::Budget { tokens });
            }
            if let Some(effort) = map.get("effort").and_then(serde_json::Value::as_str) {
                return parse_reasoning_name(effort);
            }
            Err("expected `auto`, `off`, an effort object or a budget object".to_owned())
        }
        _ => Err("expected a string, a number or an object".to_owned()),
    }
}

/// Parses `auto`, `off` or one of Norte's effort names.
fn parse_reasoning_name(text: &str) -> Result<ProtocolReasoning, String> {
    let normalized = text.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "auto" | "automatic" => Ok(ProtocolReasoning::Automatic),
        "off" | "none" | "disabled" => Ok(ProtocolReasoning::Disabled),
        _ => parse_reasoning_effort(text)
            .map(|effort| ProtocolReasoning::Effort {
                effort: effort_name(effort).to_owned(),
            })
            .ok_or_else(|| {
                format!(
                    "unknown reasoning setting `{text}`; use `auto`, `off`, `minimal`, `low`, `medium`, `high`, `xhigh` or `max`"
                )
            }),
    }
}

/// Maps an effort name to Norte's enum.
fn parse_reasoning_effort(name: &str) -> Option<ReasoningEffort> {
    match name.trim().to_ascii_lowercase().as_str() {
        "minimal" => Some(ReasoningEffort::Minimal),
        "low" => Some(ReasoningEffort::Low),
        "medium" => Some(ReasoningEffort::Medium),
        "high" => Some(ReasoningEffort::High),
        "xhigh" => Some(ReasoningEffort::XHigh),
        "max" => Some(ReasoningEffort::Max),
        _ => None,
    }
}

/// Checks one setting against the model's declared reasoning capabilities.
fn validate_reasoning(
    setting: &ProtocolReasoning,
    capabilities: &ReasoningCapability,
) -> Result<ReasoningRequest, ServiceError> {
    match setting {
        ProtocolReasoning::Automatic => Ok(ReasoningRequest::Automatic),
        ProtocolReasoning::Disabled => {
            if capabilities.supported && !capabilities.can_disable {
                return Err(ServiceError::Reasoning(
                    "the model does not allow turning reasoning off".to_owned(),
                ));
            }
            Ok(ReasoningRequest::Disabled)
        }
        ProtocolReasoning::Effort { effort } => {
            if !capabilities.supported {
                return Err(ServiceError::Reasoning(
                    "the model does not support reasoning; omit the setting or use `auto`"
                        .to_owned(),
                ));
            }
            let Some(effort) = parse_reasoning_effort(effort) else {
                return Err(ServiceError::Reasoning(format!(
                    "unknown effort `{effort}`; the model supports {}",
                    effort_choices(capabilities)
                )));
            };
            if !capabilities.efforts.contains(&effort) {
                return Err(ServiceError::Reasoning(format!(
                    "the model does not support effort `{}`; it supports {}",
                    effort_name(effort),
                    effort_choices(capabilities)
                )));
            }
            Ok(ReasoningRequest::Effort(effort))
        }
        ProtocolReasoning::Budget { tokens } => {
            if !capabilities.supported || !capabilities.supports_budget {
                return Err(ServiceError::Reasoning(
                    "the model does not accept a reasoning token budget".to_owned(),
                ));
            }
            if *tokens == 0 {
                return Err(ServiceError::Reasoning(
                    "the reasoning budget must be at least one token".to_owned(),
                ));
            }
            if let Some(max) = capabilities.max_budget {
                if *tokens > max {
                    return Err(ServiceError::Reasoning(format!(
                        "the model accepts at most {max} reasoning tokens"
                    )));
                }
            }
            Ok(ReasoningRequest::Budget(*tokens))
        }
    }
}

/// The effort names a model accepts, for an error message.
fn effort_choices(capabilities: &ReasoningCapability) -> String {
    if capabilities.efforts.is_empty() {
        return "no explicit efforts (use `auto`)".to_owned();
    }
    capabilities
        .efforts
        .iter()
        .map(|effort| format!("`{}`", effort_name(*effort)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn effort_name(effort: ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::Minimal => "minimal",
        ReasoningEffort::Low => "low",
        ReasoningEffort::Medium => "medium",
        ReasoningEffort::High => "high",
        ReasoningEffort::XHigh => "xhigh",
        ReasoningEffort::Max => "max",
        // A future level has no host spelling until this crate learns it.
        _ => "other",
    }
}

impl Service {
    /// The models the picker shows, as an event.
    pub(super) fn list_models_event(&self, request_id: u64) -> EventStream {
        single(
            request_id,
            None,
            ServerEventKind::ModelList {
                models: self.list_models(),
            },
        )
    }

    /// Flattens the catalogue for a picker, applying the config overrides.
    pub(super) fn list_models(&self) -> Vec<ModelInfo> {
        model_list(&self.config, &self.catalog())
    }

    /// Refreshes the catalogue from where it came from.
    pub(super) async fn refresh_catalog(
        &self,
        request_id: u64,
    ) -> Result<EventStream, ServiceError> {
        let loaded = load_catalog(&LoadOptions {
            url: &self.catalog_settings.url,
            ttl: self.catalog_settings.ttl,
            offline: self.catalog_settings.offline,
            cache_dir: &self.catalog_settings.cache_dir,
        })
        .await?;
        let models = loaded.catalog.model_count();
        *self.lock_catalog_write() = Arc::new(loaded.catalog);
        Ok(single(
            request_id,
            None,
            ServerEventKind::CatalogUpdated { models },
        ))
    }

    /// Switches the session's model, or queues the switch for the next turn
    /// boundary when a live turn owns it.
    pub(super) async fn set_model_event(
        &self,
        request_id: u64,
        session: SessionKey,
        model: String,
    ) -> Result<EventStream, ServiceError> {
        let spec = self.resolve_model_switch(&session, &model).await?;
        match self.set_model(&session, spec).await {
            Ok(()) => Ok(single(
                request_id,
                Some(session),
                ServerEventKind::ModelChanged { model },
            )),
            // A live turn owns the boundary. Accept the request now and keep it
            // durable; `start_turn` applies it when the session is idle.
            Err(ServiceError::Unsupported(_)) => {
                {
                    let mut sessions = self.lock_sessions();
                    let record = sessions
                        .get_mut(&session)
                        .ok_or_else(|| ServiceError::UnknownSession(session.to_string()))?;
                    record.pending_model = Some(model.clone());
                }
                self.persist(&session).await?;
                Ok(single(
                    request_id,
                    Some(session),
                    ServerEventKind::ModelQueued { model },
                ))
            }
            Err(error) => Err(error),
        }
    }

    /// Resolves a model reference and checks it against the stored reasoning
    /// and budget before the switch is queued or applied.
    pub(super) async fn resolve_model_switch(
        &self,
        session: &SessionKey,
        model: &str,
    ) -> Result<ModelSpec, ServiceError> {
        let spec = ModelSpec::from_catalog(&self.config, &self.catalog(), model, None)?;
        self.ensure_model_supports_reasoning(session, &spec)?;
        self.ensure_model_supports_budget(session, &spec).await?;
        Ok(spec)
    }

    /// Blocks a switch that would make a stored reasoning setting unsupported.
    pub(super) fn ensure_model_supports_reasoning(
        &self,
        key: &SessionKey,
        spec: &ModelSpec,
    ) -> Result<(), ServiceError> {
        let (session_reasoning, pending_reasoning) = {
            let sessions = self.lock_sessions();
            let record = sessions
                .get(key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            (record.reasoning.clone(), record.pending_reasoning.clone())
        };
        for setting in [session_reasoning.as_ref(), pending_reasoning.as_ref()]
            .into_iter()
            .flatten()
        {
            resolve_reasoning(
                Some(setting),
                &self.config.reasoning.default,
                &spec.capabilities.reasoning,
            )?;
        }
        Ok(())
    }

    pub(super) async fn set_model(
        &self,
        key: &SessionKey,
        spec: ModelSpec,
    ) -> Result<(), ServiceError> {
        let (nid, _, phase) = self.require_record(key)?;
        if phase != norte_code_protocol::Phase::Idle {
            return Err(ServiceError::Unsupported(
                "switch the model at a turn boundary",
            ));
        }
        let engine = self.engine(&spec)?;
        if engine.session(nid).await?.phase != norte::Phase::Idle {
            return Err(ServiceError::Unsupported(
                "switch the model at a turn boundary",
            ));
        }
        {
            let mut lock = self.lock_sessions();
            let record = lock
                .get_mut(key)
                .ok_or_else(|| ServiceError::UnknownSession(key.to_string()))?;
            record.spec = spec;
            record.pending_model = None;
        }
        self.persist(key).await
    }

    /// Applies a queued model switch when the session reaches an idle turn
    /// boundary. An approval or question continuation is not a boundary, so it
    /// leaves the request queued for the next real turn.
    pub(super) async fn apply_pending_model(
        &self,
        request_id: u64,
        key: &SessionKey,
    ) -> Result<Option<ServerEvent>, ServiceError> {
        let Some(model) = self
            .lock_sessions()
            .get(key)
            .and_then(|record| record.pending_model.clone())
        else {
            return Ok(None);
        };
        let spec = match self.resolve_model_switch(key, &model).await {
            Ok(spec) => spec,
            Err(error) => {
                // The queued switch cannot be honoured anymore: the model left
                // the catalogue, or the session's settings refuse it. The turn
                // goes on with the current model; failing here would break
                // every turn of the session from now on.
                tracing::warn!(
                    session = %key,
                    model = %model,
                    %error,
                    "dropping a queued model switch that cannot be honoured"
                );
                self.clear_pending_model(key).await;
                return Ok(None);
            }
        };
        match self.set_model(key, spec).await {
            Ok(()) => Ok(Some(ServerEvent::new(
                request_id,
                Some(key.clone()),
                ServerEventKind::ModelChanged { model },
            ))),
            Err(ServiceError::Unsupported(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Forgets a queued model switch.
    pub(super) async fn clear_pending_model(&self, key: &SessionKey) {
        {
            let mut sessions = self.lock_sessions();
            let Some(record) = sessions.get_mut(key) else {
                return;
            };
            record.pending_model = None;
        }
        if let Err(error) = self.persist(key).await {
            tracing::warn!(session = %key, %error, "could not store the dropped model switch");
        }
    }
}
