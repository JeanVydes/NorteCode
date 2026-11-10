//! One of the service's domains, as `impl Service` blocks.
//!
//! Child modules see the service's private fields, so a domain owns its
//! behaviour here while the state stays in one place.

use std::collections::BTreeMap;
use std::sync::{Arc, PoisonError};

use norte::{AnyFunction, ToolArgs, ToolDeclaration, ToolRegistry};
use norte_code_protocol::skills::{
    SkillResourceTool, SkillScriptConfig, SkillScriptTool, SkillTool,
};
use norte_code_protocol::tools::{AskUserTool, GrepTool, TodoTool, WebFetchTool};
use norte_tools::{
    BrowserTool, EditTool, ReadTool, ShellLimits, ShellRequest, ShellTool, WriteTool,
};

use norte_code_wire::SessionKey;

use crate::ServiceError;

use self::screenshots::publishing_tool;
use self::subagents::{SubagentRunner, SubagentTool};
use background::ShellCalls;
use effects::journalled;
use manage::ManageJobs;
use plans::PlanTool;
use sandbox::sandboxed_command;
use tooling::recording_tool;
use workspace::{Confined, PathKind, Workspace, confined};

#[allow(clippy::wildcard_imports)]
use super::*;
pub use approvals::ToolStatus;

impl Service {
    /// Registers one rich tool with the activity recorder attached.
    ///
    /// A tool the configuration disables is not registered at all: it is not in
    /// the schema the model is handed, not in the request, and not in the
    /// registry a call is looked up in. Refusing the call after the model made
    /// it would cost a round trip and teach it nothing, and the declarations are
    /// what the prompt pays for on every request.
    ///
    /// The name is read from the declaration, which is the name the model calls,
    /// so one list covers the host's own tools and the MCP servers' alike.
    pub(super) fn register_tool<A>(
        &self,
        tools: &mut ToolRegistry,
        disabled: &[String],
        declaration: ToolDeclaration<A>,
    ) -> Result<(), ServiceError>
    where
        A: ToolArgs + serde::de::DeserializeOwned + std::fmt::Debug + 'static,
    {
        if withheld(disabled, declaration.name) {
            return Ok(());
        }
        let name = declaration.name;
        // Two gates: whether a crash may already have run the call, then the
        // workspace confine. Approval rules are evaluated before calls run by
        // the engine's `ToolRuleInterceptor`.
        let declaration = journalled(declaration, Arc::clone(&self.effects));
        let recorded = recording_tool(declaration, Arc::clone(&self.activity));
        let tool = confined(Arc::new(recorded), self.confine(name));
        tools.register(tool)?;
        Ok(())
    }
    /// The path arguments a tool names, and what they are expected to be.
    ///
    /// A tool the host does not know gets no confine: a rule naming a path
    /// field of a tool nobody here wrote would be a guess, and an MCP tool's
    /// paths are its server's business.
    pub(super) fn confine(&self, tool: &str) -> Workspace {
        let enforce_root = self.config().tools.confinement.workspace_only;
        let fields = match tool {
            "read" | "write" | "edit" => vec![Confined {
                key: "path",
                default_to_root: false,
                kind: PathKind::File,
            }],
            "grep" => vec![Confined {
                key: "path",
                default_to_root: true,
                kind: PathKind::FileOrDirectory,
            }],
            "shell" => vec![Confined {
                key: "cwd",
                default_to_root: false,
                kind: PathKind::Directory,
            }],
            _ => Vec::new(),
        };
        Workspace {
            root: self.workspace_root.clone(),
            enforce_root,
            fields,
        }
    }
    /// The tools withheld from a session.
    ///
    /// A session that exists answers with the list it was made with, which is
    /// what keeps its prompt prefix stable. One that does not exist yet — the
    /// engine a session is created through, before there is a record to freeze
    /// — answers with the configuration, which is the list that is about to be
    /// frozen into it.
    pub(super) fn disabled_for(&self, session: Option<&SessionKey>) -> Vec<String> {
        session
            .and_then(|key| {
                self.lock_sessions()
                    .get(key)
                    .map(|record| record.disabled_tools.clone())
            })
            .unwrap_or_else(|| self.configured_disabled_tools())
    }
    /// The shell, and the manager for the jobs it can leave behind.
    ///
    /// One shell with one set of limits: the sandbox, the secrets, the output
    /// bounds and the time bound are the same whether a call is waited for or let
    /// go of, and `detach` on the request is what says which.
    pub(super) fn register_shell(
        &self,
        tools: &mut ToolRegistry,
        disabled: &[String],
    ) -> Result<(), ServiceError> {
        let mut env = BTreeMap::new();
        if let Ok(path) = std::env::var("PATH") {
            env.insert("PATH".to_owned(), path);
        }
        // Detached jobs are bounded too: RAEP requires every job to have a
        // runtime ceiling, and a requested zero falls back to the executor's
        // default timeout rather than creating an unbounded process.
        let limits = ShellLimits {
            max_model_bytes: self.config().tools.command_max_model_bytes,
            allow_indefinite: false,
            ..ShellLimits::default()
        };
        let secrets = self
            .secrets
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        // One builder for both, so nothing but the flag can drift apart.
        let build = |limits: ShellLimits| -> ToolDeclaration<ShellRequest> {
            let declaration = ShellTool::new()
                .with_limits(limits)
                .with_env(env.clone())
                .with_secrets(secrets.clone())
                .with_blobs(Arc::clone(&self.blobs))
                .into_declaration();
            if self.config().tools.confinement.sandbox.enabled {
                sandboxed_command(
                    declaration,
                    &self.workspace_root,
                    &self.config().tools.confinement.sandbox,
                )
            } else {
                declaration
            }
        };
        // One shell. Whether a call holds the turn or is let go of is the
        // request's `detach` field, so there is nothing here to keep in step:
        // the sandbox, the secrets, the output bounds and the limits are one
        // set, and the flag is the only thing that differs.
        let command = build(limits);
        let command = ShellCalls::declaration_with_limits(
            command,
            limits,
            Arc::clone(&self.jobs),
            Arc::clone(&self.steering),
            Arc::clone(&self.detaches),
            Arc::clone(&self.job_host),
        );
        self.register_tool(tools, disabled, command)?;
        self.register_tool(
            tools,
            disabled,
            ManageJobs::new(Arc::clone(&self.jobs)).into_declaration(),
        )?;
        Ok(())
    }
    /// Registers the delegating tools (RFC 8.2.5–8.2.9) over the shared
    /// child registry: they are what the parent uses to look at, talk to,
    /// stop and wait for the children `spawn_agent` made.
    pub(super) fn register_delegator_tools(
        &self,
        tools: &mut ToolRegistry,
        disabled: &[String],
    ) -> Result<(), ServiceError> {
        use subagents::{
            ApproveChildCall, AskDelegator, HandoffAgent, InspectAgent, ListAgents, MessageAgent,
            StopAgent, WaitAgent,
        };
        let children = Arc::clone(&self.children);
        self.register_tool(
            tools,
            disabled,
            InspectAgent::new(Arc::clone(&children), Arc::clone(&self.job_host)).into_declaration(),
        )?;
        self.register_tool(
            tools,
            disabled,
            MessageAgent::new(Arc::clone(&children), Arc::clone(&self.job_host)).into_declaration(),
        )?;
        self.register_tool(
            tools,
            disabled,
            StopAgent::new(Arc::clone(&children), Arc::clone(&self.job_host)).into_declaration(),
        )?;
        self.register_tool(
            tools,
            disabled,
            WaitAgent::new(Arc::clone(&children), Arc::clone(&self.job_host)).into_declaration(),
        )?;
        self.register_tool(
            tools,
            disabled,
            ListAgents::new(Arc::clone(&children), Arc::clone(&self.job_host)).into_declaration(),
        )?;
        self.register_tool(
            tools,
            disabled,
            ApproveChildCall::new(Arc::clone(&children), Arc::clone(&self.questions))
                .into_declaration(),
        )?;
        self.register_tool(
            tools,
            disabled,
            AskDelegator::new(Arc::clone(&self.questions)).into_declaration(),
        )?;
        self.register_tool(
            tools,
            disabled,
            HandoffAgent::new(Arc::clone(&self.job_host)).into_declaration(),
        )?;
        Ok(())
    }
    /// The tool registry every engine shares, rebuilt cheaply per turn because
    /// Norte's registry is immutable and not cloneable.
    ///
    /// Public so a host can report what the model can reach without asking the
    /// engine, which is what `/debug` shows.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] when a declaration is invalid or a name is
    /// already taken.
    #[allow(clippy::too_many_lines)]
    pub fn tools(&self, session: Option<&SessionKey>) -> Result<ToolRegistry, ServiceError> {
        let disabled = self.disabled_for(session);
        let mut tools = ToolRegistry::new();
        self.register_tool(&mut tools, &disabled, ReadTool::new().into_declaration())?;
        self.register_tool(&mut tools, &disabled, WriteTool::new().into_declaration())?;
        self.register_tool(&mut tools, &disabled, EditTool::new().into_declaration())?;
        self.register_tool(
            &mut tools,
            &disabled,
            PlanTool::new(Arc::clone(&self.plans), Arc::clone(&self.questions)).into_declaration(),
        )?;
        // The browser's capture is promoted into a user message: the tool
        // answers with the PNG as base64 in its result, and a tool result
        // carries text only on the chat completions wire. The wrapper reads the
        // capture out before the model sees the answer, and the interceptor
        // below attaches it.
        let browser = BrowserTool::new().into_declaration();
        let browser = if self.config().tools.screenshots.enabled {
            publishing_tool(
                browser,
                Arc::clone(&self.screenshots),
                Arc::clone(&self.blobs),
                screenshots::preprocessor(&self.config().tools.screenshots),
            )
        } else {
            browser
        };
        self.register_tool(&mut tools, &disabled, browser)?;

        self.register_shell(&mut tools, &disabled)?;
        self.register_tool(
            &mut tools,
            &disabled,
            GrepTool::new(&self.workspace_root).into_declaration(),
        )?;
        self.register_tool(
            &mut tools,
            &disabled,
            TodoTool::new(Arc::clone(&self.todo)).into_declaration(),
        )?;
        self.register_tool(
            &mut tools,
            &disabled,
            AskUserTool::new(Arc::clone(&self.questions)).into_declaration(),
        )?;
        let runner: Arc<dyn SubagentRunner> = self
            .subagent_runner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .unwrap_or_else(|| Arc::new(SubagentRunnerImpl::new(None)) as Arc<dyn SubagentRunner>);
        let subagent = SubagentTool::with_children(
            runner,
            Arc::clone(&self.jobs),
            Arc::clone(&self.steering),
            Arc::clone(&self.detaches),
            Arc::clone(&self.children),
            Arc::clone(&self.job_host),
            3,
        );
        self.register_tool(
            &mut tools,
            &disabled,
            subagent.into_delegation_declaration(),
        )?;
        self.register_delegator_tools(&mut tools, &disabled)?;

        if self.config().tools.web_fetch {
            self.register_tool(
                &mut tools,
                &disabled,
                WebFetchTool::new().into_declaration(),
            )?;
        }

        if self.config().skills.enabled {
            let skills = self.skills();
            self.register_tool(
                &mut tools,
                &disabled,
                SkillTool::new(Arc::clone(&skills)).into_declaration(),
            )?;
            self.register_tool(
                &mut tools,
                &disabled,
                SkillResourceTool::new(Arc::clone(&skills)).into_declaration(),
            )?;
            if self.config().skills.allow_scripts {
                self.register_tool(
                    &mut tools,
                    &disabled,
                    SkillScriptTool::new(
                        Arc::clone(&skills),
                        SkillScriptConfig::in_workspace(&self.workspace_root),
                    )
                    .into_declaration(),
                )?;
            }
        }

        // The MCP tools were wrapped when the servers connected; registering
        // them here is what puts them in front of the model.
        let mcp_tools: Vec<Arc<dyn AnyFunction>> = self
            .mcp_servers
            .read()
            .map(|servers| servers.tools())
            .unwrap_or_default();
        for tool in mcp_tools {
            // An MCP tool is named by its server — `server__tool` — and the
            // configuration disables it by that same name, so one list covers
            // the host's tools and the servers' alike.
            if withheld(&disabled, tool.name()) {
                continue;
            }
            tools.register(tool)?;
        }
        Ok(tools)
    }
    /// Every tool the model can reach, with how a call to it is decided.
    ///
    /// The registry is the list, so a tool that failed to register is absent
    /// and one from an MCP server is present, which is what makes this the
    /// answer to "what can the model do right now". Each entry carries whether
    /// a call to it runs and where that rule comes from: `deny` before `allow`,
    /// which is the order the gate applies, and asking the service rather than
    /// reading the rules in the client is what keeps the two from disagreeing.
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
        let session_id = session_id.unwrap_or_default();
        // The list a client is shown is the one the session it is on was made
        // with, so `/tools` and the gate agree about what the model can reach.
        let Ok(tools) = self.tools(session) else {
            return Vec::new();
        };
        let mut statuses: Vec<ToolStatus> = tools
            .specs()
            .into_iter()
            .map(|spec| {
                let name = spec.name.to_string();
                let (allowed, scope) = self.approval_rules.status(session_id, &name);
                ToolStatus {
                    name,
                    allowed,
                    scope,
                }
            })
            .collect();
        statuses.sort_by(|left, right| left.name.cmp(&right.name));
        statuses
    }
    /// Sets one rule for a session or the user config.
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
    /// Forgets a tool's rule.
    ///
    /// It is the third state a rule panel offers: a tool is allowed or denied,
    /// and "no rule" is what is left when neither is written down. The rule is
    /// removed from both sides — the live registry and the session or the
    /// config file — because a rule forgotten in memory and kept on disk comes
    /// back at the next restart.
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
    /// Writes one tool-wide rule to the user config and applies it in memory.
    pub(super) fn remember_global_rule(&self, tool: &str, allow: bool) -> Result<(), ServiceError> {
        let path = self
            .config_path
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .or_else(norte_code_protocol::config::user_config_path)
            .ok_or_else(|| {
                ServiceError::Approval(
                    "no user config path; set --config before choosing a global rule".to_owned(),
                )
            })?;
        norte_code_protocol::config::update_user_approval(&path, tool, allow)?;
        if allow {
            self.approval_rules.allow_global(tool);
        } else {
            self.approval_rules.deny_global(tool);
        }
        Ok(())
    }
    /// Removes one tool-wide rule from the user config and forgets it in memory.
    ///
    /// It is what "ask again" means: the rule is dropped from the file and from
    /// the live registry together, so a call to the tool is decided by the mode
    /// from the next turn on and the rule does not come back at the next start.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Approval`] when there is no config path to write
    /// to, which is the same condition a global rule has.
    pub(super) fn forget_global_rule(&self, tool: &str) -> Result<(), ServiceError> {
        let path = self
            .config_path
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .or_else(norte_code_protocol::config::user_config_path)
            .ok_or_else(|| {
                ServiceError::Approval(
                    "no user config path; set --config before choosing a global rule".to_owned(),
                )
            })?;
        norte_code_protocol::config::forget_user_approval(&path, tool)?;
        self.approval_rules.forget_global(tool);
        Ok(())
    }
}
