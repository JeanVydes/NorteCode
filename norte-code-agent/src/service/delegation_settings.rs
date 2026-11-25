//! Resolving delegated agent settings and building bounded parent context.

use std::sync::{Arc, Mutex, PoisonError};

use norte_code_protocol::config::Config;
use norte_code_protocol::costs::BudgetBook;
use norte_code_wire::SessionKey;

use crate::ServiceError;
use crate::provider::ModelSpec;

use super::Service;

pub(super) struct SubagentSettings {
    pub(super) agent_id: String,
    pub(super) model: ModelSpec,
    pub(super) disabled_tools: Vec<String>,
    pub(super) budget: Arc<Mutex<BudgetBook>>,
    pub(super) system_prompt: String,
}

impl Service {
    /// Resolves the model and tool policy for a child session.
    pub(super) async fn subagent_settings(
        &self,
        parent: &SessionKey,
        requested_agent: Option<&str>,
    ) -> Result<SubagentSettings, ServiceError> {
        let (_, parent_spec, _) = self.require_record(parent)?;
        let parent_spec =
            parent_spec.ok_or_else(|| ServiceError::UnknownSession(parent.to_string()))?;
        let (parent_profile, agent_id, target_config) =
            self.delegation_profile(parent, requested_agent)?;
        let spec = match (
            target_config.default_provider.as_deref(),
            target_config.default_model.as_deref(),
        ) {
            (Some(provider), Some(model)) if agent_id != parent_profile => {
                self.default_spec_for(&target_config, &self.lock_catalog(), provider, model)?
            }
            _ => parent_spec,
        };
        let disabled_tools = self.child_disabled_tools(parent, &target_config);
        let budget = self.budget_book(parent).await?;
        let system_prompt = target_config
            .system_prompt
            .unwrap_or_else(|| self.subagent_system_prompt.clone());
        Ok(SubagentSettings {
            agent_id,
            model: spec,
            disabled_tools,
            budget,
            system_prompt,
        })
    }

    fn delegation_profile(
        &self,
        parent: &SessionKey,
        requested_agent: Option<&str>,
    ) -> Result<(String, String, Config), ServiceError> {
        let root_config = self.config();
        let profiles = self
            .agent_profiles
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        let parent_profile = self
            .lock_sessions()
            .get(parent)
            .and_then(|record| record.agent_profile.clone())
            .unwrap_or_else(|| root_config.name.clone());
        let parent_config = profiles
            .get(&parent_profile)
            .cloned()
            .unwrap_or(root_config);
        let agent_id = requested_agent
            .or(parent_config.default_subagent_delegation.as_deref())
            .unwrap_or(&parent_profile)
            .to_owned();
        if !parent_config
            .delegates
            .iter()
            .any(|delegate| delegate == &agent_id)
        {
            let allowed = if parent_config.delegates.is_empty() {
                "none (delegation is disabled for this profile; `delegates` in the agent profile enables it)".to_owned()
            } else {
                format!("[{}]", parent_config.delegates.join(", "))
            };
            return Err(ServiceError::Delegation(format!(
                "DELEGATE_NOT_ALLOWED: agent `{agent_id}` is not in `{parent_profile}`'s delegates; allowed: {allowed}"
            )));
        }
        let target_config = profiles.get(&agent_id).cloned().ok_or_else(|| {
            ServiceError::Delegation(format!(
                "AGENT_NOT_FOUND: configured child agent `{agent_id}` does not exist"
            ))
        })?;
        Ok((parent_profile, agent_id, target_config))
    }

    fn child_disabled_tools(&self, parent: &SessionKey, target_config: &Config) -> Vec<String> {
        let parent_disabled = self
            .lock_sessions()
            .get(parent)
            .map(|record| record.disabled_tools.clone())
            .unwrap_or_default();
        let mut disabled = parent_disabled;
        disabled.extend(target_config.tools.disabled.iter().cloned());
        disabled.extend(["ask_user".to_owned()]);
        // Nesting is bounded by the depth check in `spawn_agent` and, for the
        // internal `subagent` tool, by not registering it on a child at all:
        // a child keeps `spawn_agent` (which enforces the depth) but never
        // the internal name, so the two spellings cannot drift apart at the
        // limit. The child's depth is always one past its parent's, so a
        // child (depth >= 1) never sees `subagent`.
        disabled.extend(["subagent".to_owned()]);
        let max_depth = u32::try_from(self.delegation_limits().max_depth).unwrap_or(u32::MAX);
        if self.depth_of(parent) >= max_depth {
            disabled.extend(["spawn_agent".to_owned()]);
        }
        disabled.sort();
        disabled.dedup();
        disabled
    }

    /// The parent profile's `delegates` list, resolved the same way
    /// `subagent_settings` resolves the child, so tools can show the model
    /// exactly which profiles it may spawn.
    pub(super) fn delegates_for(&self, parent: &SessionKey) -> Vec<String> {
        let root_config = self.config();
        let profiles = self
            .agent_profiles
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let parent_profile = self
            .lock_sessions()
            .get(parent)
            .and_then(|record| record.agent_profile.clone())
            .unwrap_or_else(|| root_config.name.clone());
        profiles.get(&parent_profile).map_or_else(
            || root_config.delegates.clone(),
            |config| config.delegates.clone(),
        )
    }

    pub(super) async fn delegation_context_summary(
        &self,
        parent: &SessionKey,
    ) -> Result<String, ServiceError> {
        let (nid, spec, _) = self.require_record(parent)?;
        let spec = spec.ok_or_else(|| ServiceError::UnknownSession(parent.to_string()))?;
        let history = self.engine(&spec, Some(parent))?.history(nid).await?;
        let secrets = self
            .secrets
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let mut lines = Vec::new();
        let mut chars = 0usize;
        for message in history.iter().rev() {
            if !matches!(message.role, norte::Role::User | norte::Role::Assistant) {
                continue;
            }
            let role = match message.role {
                norte::Role::User => "user",
                norte::Role::Assistant => "assistant",
                _ => continue,
            };
            let mut text = norte::Message::text(message);
            for secret in &secrets {
                if !secret.is_empty() {
                    text = text.replace(secret, "[redacted]");
                }
            }
            let line = format!("{role}: {}", text.trim());
            let remaining = 4096usize.saturating_sub(chars);
            if remaining == 0 {
                break;
            }
            let line: String = line.chars().take(remaining).collect();
            chars += line.chars().count();
            lines.push(line);
        }
        lines.reverse();
        Ok(lines.join("\n"))
    }
}
