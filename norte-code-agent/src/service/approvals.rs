//! The rules that decide whether a tool call may run.
//!
//! A rule is a static gate, applied where the call runs: a tool named in `deny`
//! is refused before it does anything, and the refusal reaches the model as a
//! tool result it reads exactly like a failure, so it can adapt. There is no
//! prompt and no parked turn — the engine has no phase to wait for a person in —
//! and therefore no grant to issue either: what a person allows is written down
//! as a rule, and a rule outlives the turn that asked for it.
//!
//! The gate is a wrapper around the declaration, applied in the one place every
//! tool passes through, so a rule and the list a client draws cannot disagree:
//! both read the same [`ApprovalRules`].

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError};

use norte::{FnExecutor, Result, SessionId, ToolArgs, ToolContext, ToolDeclaration, ToolOutput};
use norte_code_protocol::config::ApprovalsConfig;

/// Where a rule comes from.
///
/// It is what tells a rule somebody wrote from the one every session starts
/// with, which is the whole reason the tool list is worth asking for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleScope {
    /// This session only, set with `/allow <tool>` or `/deny <tool>`.
    Session,
    /// Every session, kept in the user config.
    Global,
    /// Nobody wrote a rule: the tool runs.
    Default,
}

/// How a tool is decided, and by which rule.
///
/// The client asks for this rather than reading the rules itself: `deny` wins
/// over `allow`, and which of the two applies is the whole question a person
/// opening the tool list is asking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolStatus {
    /// The name the model calls it by.
    pub name: String,
    /// Whether a call to it runs.
    pub allowed: bool,
    /// Where the rule comes from.
    pub scope: RuleScope,
}

/// Global and per-session rules.
///
/// Global rules live in the config file; session rules are mirrored into the
/// logical session, so a restart keeps them.
#[derive(Debug, Default)]
pub(super) struct ApprovalRules {
    global_allow: Mutex<BTreeSet<String>>,
    global_deny: Mutex<BTreeSet<String>>,
    session_allow: Mutex<BTreeMap<SessionId, BTreeSet<String>>>,
    session_deny: Mutex<BTreeMap<SessionId, BTreeSet<String>>>,
}

impl ApprovalRules {
    pub(super) fn new(config: &ApprovalsConfig) -> Self {
        Self {
            global_allow: Mutex::new(config.allow.iter().cloned().collect()),
            global_deny: Mutex::new(config.deny.iter().cloned().collect()),
            session_allow: Mutex::new(BTreeMap::new()),
            session_deny: Mutex::new(BTreeMap::new()),
        }
    }

    /// Whether a rule refuses this tool.
    ///
    /// Denial wins, and a deny in either list is a deny: a tool named in both
    /// is a tool somebody changed their mind about, and refusing is the answer
    /// that cannot surprise anyone.
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

    /// Whether a rule lets this tool run.
    ///
    /// It is the other half of [`Self::denies`], and it exists because the two
    /// questions a caller asks are not the same: a rule panel wants to know
    /// whether something was allowed on purpose, and a call wants to know
    /// whether it may run.
    #[cfg(test)]
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

    /// Forgets every global rule a tool has.
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

    /// The global rules, as the lists the config file holds.
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

    /// Replaces a session's rules with what was persisted for it.
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

    /// Where a tool's rule comes from, and what it says.
    pub(super) fn status(&self, session: SessionId, tool: &str) -> (bool, RuleScope) {
        let (global_allow, global_deny) = self.global();
        if global_deny.contains(tool) {
            return (false, RuleScope::Global);
        }
        if global_allow.contains(tool) {
            return (true, RuleScope::Global);
        }
        let (session_allow, session_deny) = self.session(session);
        if session_deny.contains(tool) {
            return (false, RuleScope::Session);
        }
        if session_allow.contains(tool) {
            return (true, RuleScope::Session);
        }
        (true, RuleScope::Default)
    }
}

/// Wraps a declaration so its calls meet the rules before they run.
///
/// The gate is here rather than in an interceptor because the question is about
/// one tool's name and one session's rules, and this is the last point where
/// both are known before the call does anything.
pub(super) fn gated<A>(
    declaration: ToolDeclaration<A>,
    rules: Arc<ApprovalRules>,
) -> ToolDeclaration<A>
where
    A: ToolArgs + serde::de::DeserializeOwned + std::fmt::Debug + 'static,
{
    let name = declaration.name;
    ToolDeclaration {
        name,
        description: declaration.description,
        parameters: declaration.parameters,
        executor: Arc::new(GatedExecutor {
            inner: declaration.executor,
            rules,
            name,
        }),
    }
}

/// A tool executor that refuses a call a rule denies.
struct GatedExecutor<A> {
    inner: Arc<dyn FnExecutor<A, ToolOutput>>,
    rules: Arc<ApprovalRules>,
    name: &'static str,
}

#[async_trait::async_trait]
impl<A> FnExecutor<A, ToolOutput> for GatedExecutor<A>
where
    A: ToolArgs,
{
    async fn call(&self, args: A) -> Result<ToolOutput> {
        self.inner.call(args).await
    }

    async fn call_with_context(&self, context: &ToolContext, args: A) -> Result<ToolOutput> {
        // A call with no session cannot be matched against a session's rules,
        // and a global deny is the one rule that does not need one.
        let session = context.session.unwrap_or_default();
        if self.rules.denies(session, self.name) {
            // An error, not a quiet pass: the model is told why, in the same
            // shape it reads a failed call, and it can adapt.
            return Ok(ToolOutput::error(format!(
                "`{}` is denied by the approval rules; change the rule before calling it",
                self.name
            )));
        }
        self.inner.call_with_context(context, args).await
    }
}
