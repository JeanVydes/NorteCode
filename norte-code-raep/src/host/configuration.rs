//! RAEP session configuration.

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::*;

use super::NorteCodeRaepHost;

impl NorteCodeRaepHost {
    pub(crate) async fn agent_command(
        &self,
        frame: Envelope<Value>,
    ) -> Result<Vec<Envelope<Value>>, HostError> {
        self.require_attached(&frame.session_id).await?;
        let configure: SessionConfigurePayload = serde_json::from_value(frame.payload)?;
        let job_turn_policy = parse_job_turn_policy(configure.options.as_ref())?;
        if job_turn_policy.is_some()
            && configure
                .apply
                .is_some_and(|apply| apply != ConfigureApplyPolicy::Now)
        {
            return Err(HostError::InvalidFrame(
                "SESSION_OPTIONS_UNSUPPORTED: job_turn_policy only supports apply=now".to_owned(),
            ));
        }
        let session_id = frame.session_id;
        let session = SessionKey::new(session_id.clone());
        if let Some(agent_id) = &configure.agent_id
            && agent_id != &self.config.agent_id
        {
            return Err(HostError::InvalidFrame(format!(
                "agent_mismatch: session runs agent `{}`",
                self.config.agent_id
            )));
        }
        let override_value = configure.agent_override.unwrap_or(Value::Null);
        let (reasoning, model, budget) = if override_value.is_null() {
            (None, None, None)
        } else {
            let decoded = decode_override(&override_value)?;
            validate_override(decoded.0.as_ref(), decoded.1.as_ref(), decoded.2.as_ref())?;
            decoded
        };
        let mut output = Vec::new();
        if let Some((provider, model_id)) = model {
            output.extend(
                self.submit_service(
                    &session_id,
                    ClientRequestKind::SetModel {
                        session: session.clone(),
                        provider,
                        model: model_id,
                    },
                )
                .await?,
            );
        }
        if let Some(reasoning) = reasoning {
            output.extend(
                self.submit_service(
                    &session_id,
                    ClientRequestKind::SetReasoning {
                        session: session.clone(),
                        scope: norte_code_wire::ReasoningScope::Session,
                        reasoning,
                    },
                )
                .await?,
            );
        }
        if let Some(budget) = budget {
            output.extend(
                self.submit_service(
                    &session_id,
                    ClientRequestKind::SetBudget {
                        session: session.clone(),
                        budget,
                    },
                )
                .await?,
            );
        }
        if let Some(policy) = job_turn_policy {
            let service = self.service_for_session(&session_id)?;
            if !service.set_job_turn_policy(&session, policy) {
                return Err(HostError::InvalidFrame(
                    "session not found while applying job_turn_policy".to_owned(),
                ));
            }
        }
        if output.is_empty() {
            output.push(
                self.emit(
                    &session_id,
                    StandardFrameType::SessionConfigured,
                    payload(SessionConfiguredPayload { applied: true }),
                )
                .await?,
            );
        }
        Ok(output)
    }
}
