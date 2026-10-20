//! Approval rules and the policy that parks tools for a human decision.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError};

use norte::{Decision, Policy, PolicyView, SessionId};
use norte_code_config::ApprovalMode;
use norte_code_protocol::SessionKey;
use norte_code_skills::SkillRegistry;

use super::{Service, ServiceError};

/// Global and per-session approval rules.
///
/// Global rules live in the config file; session rules are mirrored into the
/// logical session so a restart keeps them.
#[derive(Debug, Default)]
pub(super) struct ApprovalRules {
    global_allow: Mutex<BTreeSet<String>>,
    global_deny: Mutex<BTreeSet<String>>,
    session_allow: Mutex<BTreeMap<SessionId, BTreeSet<String>>>,
    session_deny: Mutex<BTreeMap<SessionId, BTreeSet<String>>>,
}

impl ApprovalRules {
    pub(super) fn new(config: &norte_code_config::ApprovalsConfig) -> Self {
        Self {
            global_allow: Mutex::new(config.allow.iter().cloned().collect()),
            global_deny: Mutex::new(config.deny.iter().cloned().collect()),
            session_allow: Mutex::new(BTreeMap::new()),
            session_deny: Mutex::new(BTreeMap::new()),
        }
    }

    pub(super) fn denies(&self, session: SessionId, tool: &str) -> bool {
        if self
            .global_deny
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(tool)
        {
            return true;
        }
        self.session_deny
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&session)
            .is_some_and(|tools| tools.contains(tool))
    }

    pub(super) fn allows(&self, session: SessionId, tool: &str) -> bool {
        if self.denies(session, tool) {
            return false;
        }
        if self
            .global_allow
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(tool)
        {
            return true;
        }
        self.session_allow
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&session)
            .is_some_and(|tools| tools.contains(tool))
    }

    pub(super) fn allow_global(&self, tool: &str) {
        self.global_deny
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(tool);
        self.global_allow
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(tool.to_owned());
    }

    pub(super) fn deny_global(&self, tool: &str) {
        self.global_allow
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(tool);
        self.global_deny
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(tool.to_owned());
    }

    pub(super) fn allow_session(&self, session: SessionId, tool: &str) {
        self.session_deny
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(session)
            .or_default()
            .remove(tool);
        self.session_allow
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(session)
            .or_default()
            .insert(tool.to_owned());
    }

    pub(super) fn deny_session(&self, session: SessionId, tool: &str) {
        self.session_allow
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(session)
            .or_default()
            .remove(tool);
        self.session_deny
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(session)
            .or_default()
            .insert(tool.to_owned());
    }

    pub(super) fn load_session(&self, session: SessionId, denied: &[String], allowed: &[String]) {
        self.session_deny
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(session, denied.iter().cloned().collect());
        self.session_allow
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(session, allowed.iter().cloned().collect());
    }
}

/// Parks the tools a human must approve, according to the session's mode.
pub(super) struct ApprovalModePolicy {
    pub(super) mode: ApprovalMode,
    pub(super) skills: Arc<SkillRegistry>,
    pub(super) honor_allowed_tools: bool,
    pub(super) rules: Arc<ApprovalRules>,
}

impl Policy for ApprovalModePolicy {
    fn name(&self) -> &'static str {
        "approval-mode"
    }

    fn decide(&self, view: &PolicyView<'_>) -> Decision {
        let tool = view.call.name.as_str();
        if self.rules.denies(view.session.id, tool) {
            return Decision::Deny(format!(
                "`{tool}` is denied by the approval rules; change the rule before calling it"
            ));
        }
        if self.rules.allows(view.session.id, tool) {
            return Decision::Allow;
        }
        if self.honor_allowed_tools
            && self
                .skills
                .preapproves(view.session.id, tool, &view.call.arguments)
        {
            return Decision::Allow;
        }
        let needs_approval = match self.mode {
            ApprovalMode::Yolo => false,
            ApprovalMode::AutoEdit => matches!(tool, "command" | "skill_script"),
            ApprovalMode::Ask => {
                matches!(tool, "write" | "edit" | "command" | "skill_script")
            }
        };
        if needs_approval {
            Decision::RequireApproval(format!("allow `{tool}` to run with these arguments?"))
        } else {
            Decision::Allow
        }
    }
}

impl Service {
    /// Sets one approval rule for a session or the user config.
    ///
    /// A session rule is stored with the logical session, so it survives a
    /// restart. A global rule is written to the user config and replaces the
    /// opposite rule for the same tool. Setting an allow rule is also how a
    /// previous deny is revoked.
    ///
    /// # Errors
    ///
    /// Returns an error when the tool name is empty, the session is unknown, a
    /// session rule is requested without a session, or the config cannot be
    /// written.
    pub async fn set_approval_rule(
        &self,
        session: Option<&SessionKey>,
        tool: &str,
        allow: bool,
        global: bool,
    ) -> Result<(), ServiceError> {
        let tool = tool.trim();
        if tool.is_empty() {
            return Err(ServiceError::Approval(
                "the approval rule needs a tool name".to_owned(),
            ));
        }
        if global {
            self.remember_global_rule(tool, allow)?;
            return Ok(());
        }
        let session = session.ok_or_else(|| {
            ServiceError::Approval(
                "a session rule needs an open session; send a message first".to_owned(),
            )
        })?;
        let (nid, _, _) = self.require_record(session)?;
        {
            let mut records = self.lock_sessions();
            let record = records
                .get_mut(session)
                .ok_or_else(|| ServiceError::UnknownSession(session.to_string()))?;
            record.denied_tools.retain(|denied| denied != tool);
            record.allowed_tools.retain(|allowed| allowed != tool);
            if allow {
                record.allowed_tools.push(tool.to_owned());
            } else {
                record.denied_tools.push(tool.to_owned());
            }
        }
        if allow {
            self.approval_rules.allow_session(nid, tool);
        } else {
            self.approval_rules.deny_session(nid, tool);
        }
        self.persist(session).await
    }
}
