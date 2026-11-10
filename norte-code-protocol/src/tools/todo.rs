use async_trait::async_trait;
use norte::{FnExecutor, Result, SessionId, ToolContext, ToolDeclaration, ToolOutput};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// One item in the model's task list.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TodoItem {
    /// What has to be done.
    pub content: String,
    /// Where it stands.
    pub status: TodoStatus,
}

/// The state of one task.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    /// Not started.
    Pending,
    /// Being worked on.
    InProgress,
    /// Done.
    Completed,
}

impl TodoStatus {
    fn marker(self) -> &'static str {
        match self {
            Self::Pending => "[ ]",
            Self::InProgress => "[~]",
            Self::Completed => "[x]",
        }
    }
}

/// Arguments for the `subagent` tool.
///
/// The args are what the model writes; the engine drives them like any other
/// tool's. What a job does with them is the host's business —
/// `service::subagents` registers one when `detach` says to.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubagentArgs {
    /// The one task, with everything the child needs. The child starts blank.
    pub prompt: String,
    /// The child session's title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Let the run go as a job rather than waiting for it.
    #[serde(default)]
    pub detach: bool,
}

/// Arguments for `todo`.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TodoArgs {
    /// The complete list after this update; it replaces the previous list.
    pub todos: Vec<TodoItem>,
}

/// Per-session task state.
#[derive(Debug, Default)]
pub struct TodoState {
    lists: Mutex<BTreeMap<Option<SessionId>, Vec<TodoItem>>>,
}

impl TodoState {
    /// The current list for a session.
    #[must_use]
    pub fn list(&self, session: Option<SessionId>) -> Vec<TodoItem> {
        self.lock().get(&session).cloned().unwrap_or_default()
    }

    /// Moves the list from one Norte session to another, as compaction does.
    pub fn carry(&self, from: SessionId, to: SessionId) {
        let mut lists = self.lock();
        if let Some(todos) = lists.get(&Some(from)).cloned() {
            lists.insert(Some(to), todos);
        }
    }

    /// Puts a list back for a session this process has not seen before.
    ///
    /// The model writes the list with the `todo` tool, and the call it made is
    /// part of the conversation, so a host that reopens a session reads the
    /// last list back out of the history and hands it here. Without this a
    /// resumed session started with no list, and a compaction before the model
    /// wrote a new one summarised the work without the tasks it was tracking.
    pub fn restore(&self, session: SessionId, todos: Vec<TodoItem>) {
        self.replace(Some(session), todos);
    }

    /// The list rendered as the model sees it.
    #[must_use]
    pub fn text(&self, session: Option<SessionId>) -> String {
        render_todos(&self.list(session))
    }

    fn replace(&self, session: Option<SessionId>, todos: Vec<TodoItem>) {
        self.lock().insert(session, todos);
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<Option<SessionId>, Vec<TodoItem>>> {
        self.lists.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A task list the model maintains across the turn.
#[derive(Clone, Debug)]
pub struct TodoTool {
    state: Arc<TodoState>,
}

impl TodoTool {
    /// A tool backed by `state`.
    #[must_use]
    pub fn new(state: Arc<TodoState>) -> Self {
        Self { state }
    }

    /// Turns the tool into a declaration the core registry accepts.
    #[must_use]
    pub fn into_declaration(self) -> ToolDeclaration<TodoArgs> {
        ToolDeclaration::new(
            "todo",
            "Replace the task list for this session. Send the complete list every time, with a status per item. Use it before multi-step work and after each step; the user sees the list in the footer.",
            Arc::new(self),
        )
    }
}

fn render_todos(todos: &[TodoItem]) -> String {
    if todos.is_empty() {
        return "(the task list is empty)".to_owned();
    }
    let done = todos
        .iter()
        .filter(|todo| todo.status == TodoStatus::Completed)
        .count();
    let mut text = format!("{done}/{} done\n", todos.len());
    for todo in todos {
        let _ = writeln!(text, "{} {}", todo.status.marker(), todo.content);
    }
    text
}

#[async_trait]
impl FnExecutor<TodoArgs, ToolOutput> for TodoTool {
    async fn call(&self, args: TodoArgs) -> Result<ToolOutput> {
        self.state.replace(None, args.todos.clone());
        Ok(ToolOutput::text(render_todos(&args.todos)))
    }

    async fn call_with_context(&self, context: &ToolContext, args: TodoArgs) -> Result<ToolOutput> {
        self.state.replace(context.session, args.todos.clone());
        Ok(ToolOutput::text(render_todos(&args.todos)))
    }
}
