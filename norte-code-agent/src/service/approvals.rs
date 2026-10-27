//! Approval rules and the policy that parks tools for a human decision.
//!
//! It also owns what the client does with an approval: granting it, denying
//! it, revoking a grant it already gave, and listing the grants a session
//! holds.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError};

use futures_util::StreamExt;
use norte::harness::GrantScope;
use norte::harness::{Decision, Policy, PolicyView};
use norte::{ApprovalId, GrantId, SessionId};
use norte_code_config::ApprovalMode;
use norte_code_protocol::{
    GrantInfo, GrantScope as ProtocolGrantScope, ServerEventKind, SessionKey,
};
use norte_code_skills::SkillRegistry;

use super::mapping::single;
use super::{EventStream, Service, ServiceError};

/// How a tool is decided, and how far the rule that decides it reaches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolStatus {
    /// The name the model calls it by.
    pub name: String,
    /// What happens to a call to it.
    pub rule: ToolRule,
    /// Where the rule comes from, which is what tells a rule somebody set
    /// from one the approval mode implies.
    pub scope: RuleScope,
}

/// How far a rule reaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleScope {
    /// This session only, set with `/allow <tool>` or in a session approval.
    Session,
    /// Every session, kept in the user config.
    Global,
    /// No rule names it: the approval mode decides, and no command would
    /// change it without writing one.
    Mode,
}

/// How a tool is decided, and by which rule.
///
/// The client asks for this rather than reading the rules itself: `deny` wins
/// over `allow`, and which of the two applies is the whole question a person
/// opening the tool list is asking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolRule {
    /// Refused before the approval mode is consulted.
    Deny,
    /// Runs without asking.
    Allow,
    /// A human decides, when the approval mode says this tool needs one.
    Ask,
}

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

    /// Forgets every global rule a tool has, so a call to it asks again.
    ///
    /// Both lists, because a tool with an allow and a deny is a tool somebody
    /// changed their mind about: forgetting one and leaving the other is a rule
    /// nobody wrote.
    pub(super) fn forget_global(&self, tool: &str) {
        self.global_allow
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(tool);
        self.global_deny
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(tool);
    }

    /// The session's rules for a tool, forgotten.
    pub(super) fn forget_session(&self, session: SessionId, tool: &str) {
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
            .remove(tool);
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

    /// The global rules, as the tools they name.
    ///
    /// Two lists rather than one with a flag each: they are what the config
    /// file holds, and reading them back in the shape they were written is what
    /// makes the tool list agree with the file.
    pub(super) fn global(&self) -> (BTreeSet<String>, BTreeSet<String>) {
        (
            self.global_allow
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
            self.global_deny
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
        )
    }

    /// The rules that belong to one session, as the tools they name.
    pub(super) fn session(&self, session: SessionId) -> (BTreeSet<String>, BTreeSet<String>) {
        let allows = self
            .session_allow
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&session)
            .cloned()
            .unwrap_or_default();
        let denies = self
            .session_deny
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&session)
            .cloned()
            .unwrap_or_default();
        (allows, denies)
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

/// The tools whose effects a person must approve under each mode.
///
/// Named as the registry names them. This list said `command`, and no tool has
/// been called that since the shell became one tool: the mode compared against
/// a name nothing answers to, so `ask` — the default — never asked before a
/// command ran. A wrong name here is silent, which is why the test below reads
/// the names back out of the registry.
pub(super) const WRITES: [&str; 2] = ["write", "edit"];

/// The tools that run something, as the registry names them.
pub(super) const RUNS: [&str; 2] = ["shell", "skill_script"];

/// Whether the approval mode sends this tool to a human.
///
/// It is the rule [`ApprovalModePolicy`] applies, written once so the list the
/// client draws and the decision the engine makes cannot disagree: a tool shown
/// as allowed while the mode parks it is worse than no list at all.
pub(super) fn mode_needs_approval(mode: ApprovalMode, tool: &str) -> bool {
    match mode {
        ApprovalMode::Yolo => false,
        ApprovalMode::AutoEdit => RUNS.contains(&tool),
        ApprovalMode::Ask => WRITES.contains(&tool) || RUNS.contains(&tool),
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
        let needs_approval = mode_needs_approval(self.mode, tool);
        if needs_approval {
            Decision::RequireApproval(format!("allow `{tool}` to run with these arguments?"))
        } else {
            Decision::Allow
        }
    }
}

impl Service {
    /// Every tool the model can reach, with how a call to it is decided.
    ///
    /// The registry is the list, so a tool that failed to register is absent
    /// and one from an MCP server is present, which is what makes this the
    /// answer to "what can the model do right now". Each entry carries the rule
    /// that decides the call and how far that rule reaches: `deny` before
    /// `allow`, before the approval mode — the order
    /// [`ApprovalModePolicy::decide`] applies, which is the point of asking the
    /// service rather than reading the rules in the client.
    ///
    /// A tool named only by a session rule is in the list when the session is
    /// the one it was set for, and a rule naming a tool the registry does not
    /// have is left out: a list of tools is not a list of names somebody typed.
    pub fn tool_rules(&self, session: Option<&SessionKey>) -> Vec<ToolStatus> {
        // A session that is not open has no session rules, and asking for them
        // is not an error: the list is what the *next* call meets, and a session
        // that was never opened has none.
        let session_id =
            session.and_then(|key| self.lock_sessions().get(key).map(|record| record.id));
        let (global_allow, global_deny) = self.approval_rules.global();
        let (session_allow, session_deny) = session_id
            .map(|id| self.approval_rules.session(id))
            .unwrap_or_default();
        let mode = self.config().approval_mode;
        let Ok(tools) = self.tools() else {
            return Vec::new();
        };
        let mut statuses: Vec<ToolStatus> = tools
            .specs()
            .into_iter()
            .map(|spec| {
                let name = spec.name.to_string();
                let (rule, scope) = if global_deny.contains(&name) {
                    (ToolRule::Deny, RuleScope::Global)
                } else if session_deny.contains(&name) {
                    (ToolRule::Deny, RuleScope::Session)
                } else if global_allow.contains(&name) {
                    (ToolRule::Allow, RuleScope::Global)
                } else if session_allow.contains(&name) {
                    (ToolRule::Allow, RuleScope::Session)
                } else if mode_needs_approval(mode, &name) {
                    (ToolRule::Ask, RuleScope::Mode)
                } else {
                    (ToolRule::Allow, RuleScope::Mode)
                };
                ToolStatus { name, rule, scope }
            })
            .collect();
        statuses.sort_by(|left, right| left.name.cmp(&right.name));
        statuses
    }

    /// Forgets a tool's rule, so a call to it asks again.
    ///
    /// It is the third state a rule panel offers and the service has not had:
    /// a tool is allowed or denied, and "ask" is what is left when neither is
    /// written down. The rule is removed from both sides — the live registry
    /// and the session or the config file — because a rule that is forgotten in
    /// memory and kept on disk comes back at the next restart.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Approval`] when the tool is unnamed or no
    /// session is open and the rule was a session's.
    pub async fn forget_approval_rule(
        &self,
        session: Option<&SessionKey>,
        tool: &str,
        global: bool,
    ) -> Result<(), ServiceError> {
        let tool = tool.trim();
        if tool.is_empty() {
            return Err(ServiceError::Approval(
                "the approval rule needs a tool name".to_owned(),
            ));
        }
        if global {
            return self.forget_global_rule(tool);
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
        }
        self.approval_rules.forget_session(nid, tool);
        self.persist(session).await
    }

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

impl Service {
    /// Grants an approval and resumes the parked turn.
    pub(super) async fn approve(
        self: &Arc<Self>,
        request_id: u64,
        session: SessionKey,
        approval: String,
        arguments: Option<serde_json::Value>,
        scope: Option<norte_code_protocol::GrantScope>,
        remember: Option<bool>,
    ) -> Result<EventStream, ServiceError> {
        if arguments.is_some() {
            return Err(ServiceError::Unsupported(
                "editing approval arguments is not implemented for this client",
            ));
        }
        let (nid, spec, _) = self.require_record(&session)?;
        let engine = self.engine(&spec)?;
        let id: ApprovalId = approval.parse().map_err(ServiceError::Norte)?;
        let record = engine
            .pending_approvals(nid)
            .await?
            .into_iter()
            .find(|record| record.id == id)
            .ok_or(ServiceError::Norte(norte::Error::ApprovalNotFound(id)))?;
        let global = matches!(scope, Some(norte_code_protocol::GrantScope::Always));
        if global {
            self.remember_global_rule(record.tool.as_str(), true)?;
        }
        let grant = match (scope, remember) {
            (Some(norte_code_protocol::GrantScope::Always), _) => GrantScope::Global,
            (Some(norte_code_protocol::GrantScope::Session), _) => GrantScope::Session(nid),
            (Some(norte_code_protocol::GrantScope::Tool), _) | (_, Some(true)) => {
                GrantScope::Tool(record.tool.clone())
            }
            _ => GrantScope::Call(record.call_id),
        };
        engine.approve(id, grant, "user").await?;
        let head = single(
            request_id,
            Some(session.clone()),
            ServerEventKind::ApprovalResolved {
                approval: approval.clone(),
                approved: true,
            },
        );
        let resumed = self
            .start_turn(
                request_id,
                Some(session),
                None,
                Vec::new(),
                None,
                None,
                true,
            )
            .await?;
        Ok(Box::pin(head.chain(resumed)))
    }

    /// Denies an approval and resumes the parked turn.
    pub(super) async fn deny(
        self: &Arc<Self>,
        request_id: u64,
        session: SessionKey,
        approval: String,
        reason: Option<String>,
        scope: Option<norte_code_protocol::DenyScope>,
    ) -> Result<EventStream, ServiceError> {
        let (nid, spec, _) = self.require_record(&session)?;
        let engine = self.engine(&spec)?;
        let id: ApprovalId = approval.parse().map_err(ServiceError::Norte)?;
        let record = engine
            .pending_approvals(nid)
            .await?
            .into_iter()
            .find(|record| record.id == id)
            .ok_or(ServiceError::Norte(norte::Error::ApprovalNotFound(id)))?;
        let tool = record.tool.as_str();
        match scope {
            Some(norte_code_protocol::DenyScope::ToolSession) => {
                self.approval_rules.deny_session(nid, tool);
                if let Some(record) = self.lock_sessions().get_mut(&session) {
                    if !record.denied_tools.iter().any(|denied| denied == tool) {
                        record.denied_tools.push(tool.to_owned());
                    }
                }
                self.persist(&session).await?;
            }
            Some(norte_code_protocol::DenyScope::ToolGlobal) => {
                self.remember_global_rule(tool, false)?;
            }
            Some(norte_code_protocol::DenyScope::Once) | None => {}
        }
        engine.deny(id, "user", reason.unwrap_or_default()).await?;
        let head = single(
            request_id,
            Some(session.clone()),
            ServerEventKind::ApprovalResolved {
                approval: approval.clone(),
                approved: false,
            },
        );
        let resumed = self
            .start_turn(
                request_id,
                Some(session),
                None,
                Vec::new(),
                None,
                None,
                true,
            )
            .await?;
        Ok(Box::pin(head.chain(resumed)))
    }

    /// Revokes a grant the session already holds.
    pub(super) async fn revoke(
        &self,
        request_id: u64,
        session: SessionKey,
        approval: String,
        grant: String,
    ) -> Result<EventStream, ServiceError> {
        let (nid, spec, _) = self.require_record(&session)?;
        let engine = self.engine(&spec)?;
        let grant: GrantId = grant.parse().map_err(ServiceError::Norte)?;
        engine.revoke(nid, grant, "user").await?;
        Ok(single(
            request_id,
            Some(session),
            ServerEventKind::GrantRevoked {
                approval,
                grant: grant.to_string(),
            },
        ))
    }

    /// The grants a session holds, with their scope and state.
    pub(super) async fn list_grants_event(
        &self,
        request_id: u64,
        session: SessionKey,
    ) -> Result<EventStream, ServiceError> {
        let (nid, _, _) = self.require_record(&session)?;
        let mut grants = Vec::new();
        for record in self.store.approvals(nid).await? {
            for grant in record.grants {
                grants.push(GrantInfo {
                    approval: record.id.to_string(),
                    grant: grant.id.to_string(),
                    tool: record.tool.to_string(),
                    scope: match grant.scope {
                        GrantScope::Tool(_) => ProtocolGrantScope::Tool,
                        GrantScope::Session(_) => ProtocolGrantScope::Session,
                        GrantScope::Global => ProtocolGrantScope::Always,
                        // A one-call grant and any scope Norte may add later
                        // reach the client as the narrowest scope there is.
                        _ => ProtocolGrantScope::Once,
                    },
                    issued_at: grant.issued_at.to_rfc3339(),
                    revoked: !grant.is_active(),
                });
            }
        }
        Ok(single(
            request_id,
            Some(session),
            ServerEventKind::GrantList { grants },
        ))
    }
}
