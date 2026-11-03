//! Markdown-backed plans and the model's `plan` tool.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use norte::{FnExecutor, Result, SessionId, ToolContext, ToolOutput};
use norte_code_protocol::SessionKey;
use norte_code_protocol::sqlite::{SessionDirectory, StoredSession};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The lifecycle state of one plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename_all = "snake_case")]
pub enum PlanStatus {
    /// The user approved this plan.
    Active,
    /// The plan is awaiting the user's decision.
    Pending,
    /// The user rejected or canceled this plan.
    Canceled,
}

/// A plan saved as a Markdown file in its session directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// Stable plan identifier, also used as the Markdown filename.
    pub id: String,
    /// The plan text, editable by the user.
    pub content: String,
    /// Current approval state, stored in the file's front matter.
    pub status: PlanStatus,
    /// When the plan was first saved.
    pub created_at: Option<DateTime<Utc>>,
    /// When the plan was last changed.
    pub updated_at: Option<DateTime<Utc>>,
    /// Provider selected when the proposal was created.
    pub provider: Option<String>,
    /// Model selected when the proposal was created.
    pub model: Option<String>,
}

/// Filesystem access to plans. Session records remain in the database; plans
/// live separately so they can be opened and edited as Markdown.
pub(super) struct PlanRepository {
    directory: Arc<dyn SessionDirectory>,
    root: Option<PathBuf>,
    writes: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for PlanRepository {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PlanRepository")
            .finish_non_exhaustive()
    }
}

impl PlanRepository {
    pub(super) fn new(directory: Arc<dyn SessionDirectory>) -> Self {
        let root = std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join(".local").join("norte-code"));
        Self {
            directory,
            root,
            writes: tokio::sync::Mutex::new(()),
        }
    }

    #[cfg(test)]
    fn with_root(directory: Arc<dyn SessionDirectory>, root: PathBuf) -> Self {
        Self {
            directory,
            root: Some(root),
            writes: tokio::sync::Mutex::new(()),
        }
    }

    /// Creates the session's local artifact directory, even when it has no
    /// plans yet. Other session-scoped files can share this directory.
    pub(super) async fn ensure_session_folder(
        &self,
        session: &SessionKey,
    ) -> std::result::Result<(), String> {
        let root = self.root()?;
        validate_component(session.as_str(), "session id")?;
        tokio::fs::create_dir_all(root.join(session.as_str()))
            .await
            .map_err(|error| format!("could not create the session directory: {error}"))
    }

    fn root(&self) -> std::result::Result<&Path, String> {
        self.root.as_deref().ok_or_else(|| {
            "HOME is not set, so the local session artifacts directory has no location".to_owned()
        })
    }

    async fn stored_session(
        &self,
        session: SessionId,
    ) -> std::result::Result<StoredSession, String> {
        self.directory
            .sessions()
            .await
            .map_err(|error| format!("could not find the session for its plans: {error}"))?
            .into_iter()
            .find(|stored| stored.north == session)
            .ok_or_else(|| format!("session `{session}` is not stored"))
    }

    async fn plan_directory(&self, session: SessionId) -> std::result::Result<PathBuf, String> {
        let stored = self.stored_session(session).await?;
        validate_component(&stored.key, "session id")?;
        Ok(self.root()?.join(stored.key).join("plans"))
    }

    pub(super) async fn list(&self, session: SessionId) -> std::result::Result<Vec<Plan>, String> {
        let directory = self.plan_directory(session).await?;
        let mut entries = match tokio::fs::read_dir(&directory).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(format!(
                    "could not read plan directory {}: {error}",
                    directory.display()
                ));
            }
        };
        let mut plans = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|error| format!("could not list plans: {error}"))?
        {
            let file_type = entry
                .file_type()
                .await
                .map_err(|error| format!("could not inspect plan file: {error}"))?;
            if !file_type.is_file() || entry.path().extension().is_none_or(|ext| ext != "md") {
                continue;
            }
            let id = entry
                .path()
                .file_stem()
                .and_then(std::ffi::OsStr::to_str)
                .ok_or_else(|| {
                    format!(
                        "plan filename is not valid UTF-8: {}",
                        entry.path().display()
                    )
                })?
                .to_owned();
            validate_component(&id, "plan id")?;
            plans.push(read_plan(entry.path(), id).await?);
        }
        plans.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(plans)
    }

    pub(super) async fn get(
        &self,
        session: SessionId,
        id: &str,
    ) -> std::result::Result<Option<Plan>, String> {
        let path = self.plan_path(session, id).await?;
        match tokio::fs::read_to_string(&path).await {
            Ok(contents) => parse_plan(id, &contents)
                .map(Some)
                .map_err(|error| format!("could not read plan {}: {error}", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("could not read plan {}: {error}", path.display())),
        }
    }

    async fn plan_path(
        &self,
        session: SessionId,
        id: &str,
    ) -> std::result::Result<PathBuf, String> {
        validate_component(id, "plan id")?;
        Ok(self.plan_directory(session).await?.join(format!("{id}.md")))
    }

    pub(super) async fn add(
        &self,
        session: SessionId,
        plan: &Plan,
    ) -> std::result::Result<(), String> {
        let _guard = self.writes.lock().await;
        if self.get(session, &plan.id).await?.is_some() {
            return Err(format!("plan `{}` already exists", plan.id));
        }
        let stored = self.stored_session(session).await?;
        let now = Utc::now();
        let mut plan = plan.clone();
        plan.created_at = Some(now);
        plan.updated_at = Some(now);
        plan.provider = Some(stored.provider);
        plan.model = Some(stored.model);
        self.write(session, &plan).await
    }

    pub(super) async fn set_status(
        &self,
        session: SessionId,
        id: &str,
        status: PlanStatus,
    ) -> std::result::Result<(), String> {
        let _guard = self.writes.lock().await;
        let mut plan = self
            .get(session, id)
            .await?
            .ok_or_else(|| format!("no plan `{id}` in this session"))?;
        plan.status = status;
        plan.updated_at = Some(Utc::now());
        self.write(session, &plan).await
    }

    pub(super) async fn delete(
        &self,
        session: SessionId,
        id: &str,
    ) -> std::result::Result<bool, String> {
        let _guard = self.writes.lock().await;
        let path = self.plan_path(session, id).await?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(format!("could not delete plan {}: {error}", path.display())),
        }
    }

    pub(super) async fn copy_session(
        &self,
        from: SessionId,
        to: SessionId,
    ) -> std::result::Result<(), String> {
        let source = self.plan_directory(from).await?;
        let destination = self.plan_directory(to).await?;
        let mut entries = match tokio::fs::read_dir(&source).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("could not read source plans: {error}")),
        };
        tokio::fs::create_dir_all(&destination)
            .await
            .map_err(|error| format!("could not create destination plan directory: {error}"))?;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|error| format!("could not list source plans: {error}"))?
        {
            if entry
                .file_type()
                .await
                .map_err(|error| format!("could not inspect source plan: {error}"))?
                .is_file()
                && entry.path().extension().is_some_and(|ext| ext == "md")
            {
                let name = entry.file_name();
                tokio::fs::copy(entry.path(), destination.join(name))
                    .await
                    .map_err(|error| format!("could not copy a plan: {error}"))?;
            }
        }
        Ok(())
    }

    pub(super) async fn remove_session_folder(
        &self,
        session: &SessionKey,
    ) -> std::result::Result<(), String> {
        validate_component(session.as_str(), "session id")?;
        let path = self.root()?.join(session.as_str());
        match tokio::fs::remove_dir_all(&path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!(
                "could not remove local session artifacts {}: {error}",
                path.display()
            )),
        }
    }

    async fn write(&self, session: SessionId, plan: &Plan) -> std::result::Result<(), String> {
        let directory = self.plan_directory(session).await?;
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(|error| format!("could not create plan directory: {error}"))?;
        let path = self.plan_path(session, &plan.id).await?;
        let temporary = directory.join(format!(".{}.{}.tmp", plan.id, uuid::Uuid::new_v4()));
        let document = plan_markdown(plan);
        if let Err(error) = tokio::fs::write(&temporary, document).await {
            return Err(format!("could not write temporary plan file: {error}"));
        }
        if let Err(error) = tokio::fs::rename(&temporary, &path).await {
            let _ = tokio::fs::remove_file(&temporary).await;
            return Err(format!("could not save plan {}: {error}", path.display()));
        }
        Ok(())
    }
}

fn validate_component(value: &str, description: &str) -> std::result::Result<(), String> {
    if value.is_empty()
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(format!("invalid {description} `{value}`"));
    }
    Ok(())
}

const fn status_name(status: PlanStatus) -> &'static str {
    match status {
        PlanStatus::Active => "active",
        PlanStatus::Pending => "pending",
        PlanStatus::Canceled => "canceled",
    }
}

fn plan_front_matter(plan: &Plan) -> String {
    let mut front_matter = format!("---\nstatus: {}\n", status_name(plan.status));
    if let Some(title) = plan_title(&plan.content) {
        writeln!(
            front_matter,
            "title: {}",
            serde_json::to_string(title).expect("serializing a string cannot fail")
        )
        .expect("writing to a string cannot fail");
    }
    if let Some(created_at) = &plan.created_at {
        writeln!(front_matter, "created_at: {}", created_at.to_rfc3339())
            .expect("writing to a string cannot fail");
    }
    if let Some(updated_at) = &plan.updated_at {
        writeln!(front_matter, "updated_at: {}", updated_at.to_rfc3339())
            .expect("writing to a string cannot fail");
    }
    if let Some(provider) = &plan.provider {
        writeln!(
            front_matter,
            "provider: {}",
            serde_json::to_string(provider).expect("serializing a string cannot fail")
        )
        .expect("writing to a string cannot fail");
    }
    if let Some(model) = &plan.model {
        writeln!(
            front_matter,
            "model: {}",
            serde_json::to_string(model).expect("serializing a string cannot fail")
        )
        .expect("writing to a string cannot fail");
    }
    front_matter.push_str("---");
    front_matter
}

fn plan_title(content: &str) -> Option<&str> {
    let first_line = content.lines().next()?.trim();
    let title = first_line
        .strip_prefix('#')
        .map_or(first_line, |line| line.trim_start_matches('#').trim());
    (!title.is_empty()).then_some(title)
}

fn plan_markdown(plan: &Plan) -> String {
    format!("{}\n{}", plan_front_matter(plan), plan.content)
}

async fn read_plan(path: PathBuf, id: String) -> std::result::Result<Plan, String> {
    let contents = tokio::fs::read_to_string(&path)
        .await
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    parse_plan(&id, &contents)
        .map_err(|error| format!("invalid Markdown plan {}: {error}", path.display()))
}

fn parse_plan(id: &str, contents: &str) -> std::result::Result<Plan, String> {
    let front_matter = contents
        .strip_prefix("---\n")
        .ok_or_else(|| "missing YAML front matter".to_owned())?;
    let (metadata, content) = front_matter
        .split_once("\n---\n")
        .ok_or_else(|| "unterminated YAML front matter".to_owned())?;
    let status = metadata
        .lines()
        .find_map(|line| line.strip_prefix("status: "))
        .ok_or_else(|| "front matter has no `status`".to_owned())?;
    let status = match status {
        "active" => PlanStatus::Active,
        "pending" => PlanStatus::Pending,
        "canceled" => PlanStatus::Canceled,
        other => return Err(format!("unknown plan status `{other}`")),
    };
    let optional_value = |key: &str| {
        metadata
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{key}: ")))
    };
    let optional_datetime = |key: &str| -> std::result::Result<Option<DateTime<Utc>>, String> {
        optional_value(key)
            .map(|value| {
                DateTime::parse_from_rfc3339(value)
                    .map(|timestamp| timestamp.with_timezone(&Utc))
                    .map_err(|error| format!("invalid `{key}` timestamp: {error}"))
            })
            .transpose()
    };
    let optional_string = |key: &str| -> std::result::Result<Option<String>, String> {
        optional_value(key)
            .map(|value| {
                serde_json::from_str(value)
                    .or_else(|_| Ok::<_, serde_json::Error>(value.to_owned()))
                    .map_err(|error| format!("invalid `{key}` value: {error}"))
            })
            .transpose()
    };
    Ok(Plan {
        id: id.to_owned(),
        content: content.strip_prefix('\n').unwrap_or(content).to_owned(),
        status,
        created_at: optional_datetime("created_at")?,
        updated_at: optional_datetime("updated_at")?,
        provider: optional_string("provider")?,
        model: optional_string("model")?,
    })
}

/// Operations exposed by the model's `plan` tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename_all = "snake_case")]
enum PlanAction {
    /// Submit a plan for explicit user approval.
    Propose,
    /// List this session's plans without their full contents.
    List,
    /// Read one plan.
    Get,
    /// Change a plan to pending or canceled.
    SetStatus,
    /// Remove one plan.
    Delete,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct PlanArgs {
    /// Operation to perform.
    action: PlanAction,
    /// Plan identifier for get, status changes, and delete.
    #[serde(default)]
    id: Option<String>,
    /// Proposed plan text.
    #[serde(default)]
    content: Option<String>,
    /// New status; `active` is only possible through user approval.
    #[serde(default)]
    status: Option<PlanStatus>,
}

/// The model tool for plans.
pub(super) struct PlanTool {
    plans: Arc<PlanRepository>,
    questions: Arc<norte_code_protocol::tools::QuestionBroker>,
}

impl PlanTool {
    pub(super) fn new(
        plans: Arc<PlanRepository>,
        questions: Arc<norte_code_protocol::tools::QuestionBroker>,
    ) -> Self {
        Self { plans, questions }
    }

    pub(super) fn into_declaration(self) -> norte::ToolDeclaration<PlanArgs> {
        norte::ToolDeclaration::new(
            "plan",
            "Manage plans for this session. Propose submits a plan for explicit user approval. \
             List returns each plan's front matter metadata. Get returns the complete Markdown \
             file, including its front matter. SetStatus may set pending or canceled, but only \
             user approval can activate a plan. Delete removes a plan. Plans are Markdown files \
             the user can edit.",
            Arc::new(self),
        )
    }
}

#[async_trait]
impl FnExecutor<PlanArgs, ToolOutput> for PlanTool {
    async fn call(&self, args: PlanArgs) -> Result<ToolOutput> {
        Ok(ToolOutput::error(format!(
            "`plan` needs a session context for action {:?}",
            args.action
        )))
    }

    #[allow(clippy::too_many_lines)]
    async fn call_with_context(&self, context: &ToolContext, args: PlanArgs) -> Result<ToolOutput> {
        let Some(session) = context.session else {
            return Ok(ToolOutput::error("`plan` needs a session context"));
        };
        Ok(match args.action {
            PlanAction::Propose => {
                if let Some(content) = args.content.filter(|content| !content.trim().is_empty()) {
                    let plan = Plan {
                        id: uuid::Uuid::new_v4().to_string(),
                        content: content.trim().to_owned(),
                        status: PlanStatus::Pending,
                        created_at: None,
                        updated_at: None,
                        provider: None,
                        model: None,
                    };
                    if let Err(error) = self.plans.add(session, &plan).await {
                        ToolOutput::error(format!("could not save the plan: {error}"))
                    } else {
                        let answer = self
                            .questions
                            .ask(
                                context,
                                format!("NORTE_CODE_PLAN_REVIEW\n{}", plan.content),
                                vec!["Approve".to_owned(), "Reject".to_owned()],
                                false,
                            )
                            .await;
                        match answer.as_deref() {
                            Ok("Approve") => match self
                                .plans
                                .set_status(session, &plan.id, PlanStatus::Active)
                                .await
                            {
                                Ok(()) => {
                                    plan_output(
                                        &self.plans,
                                        session,
                                        &plan.id,
                                        "Plan approved and activated.",
                                    )
                                    .await
                                }
                                Err(error) => ToolOutput::error(format!(
                                    "could not approve the plan: {error}"
                                )),
                            },
                            Ok("Reject") => match self
                                .plans
                                .set_status(session, &plan.id, PlanStatus::Canceled)
                                .await
                            {
                                Ok(()) => {
                                    plan_output(&self.plans, session, &plan.id, "Plan rejected.")
                                        .await
                                }
                                Err(error) => {
                                    ToolOutput::error(format!("could not reject the plan: {error}"))
                                }
                            },
                            Ok(other) => ToolOutput::error(format!(
                                "unexpected plan approval response `{other}`; the plan remains pending"
                            )),
                            Err(error) => ToolOutput::error(format!(
                                "could not get approval: {error}; the plan remains pending"
                            )),
                        }
                    }
                } else {
                    ToolOutput::error("`propose` needs non-empty `content`")
                }
            }
            PlanAction::List => match self.plans.list(session).await {
                Ok(plans) if plans.is_empty() => ToolOutput::text("No plans in this session."),
                Ok(plans) => ToolOutput::text(
                    plans
                        .iter()
                        .map(|plan| format!("{}\n{}", plan.id, plan_front_matter(plan)))
                        .collect::<Vec<_>>()
                        .join("\n\n"),
                ),
                Err(error) => ToolOutput::error(format!("could not list plans: {error}")),
            },
            PlanAction::Get => match args.id.as_deref() {
                Some(id) => match self.plans.get(session, id).await {
                    Ok(Some(plan)) => {
                        ToolOutput::text(format!("Plan `{}`:\n{}", plan.id, plan_markdown(&plan)))
                    }
                    Ok(None) => ToolOutput::error(format!("no plan `{id}` in this session")),
                    Err(error) => ToolOutput::error(format!("could not read plan: {error}")),
                },
                None => ToolOutput::error("`get` needs an `id`"),
            },
            PlanAction::SetStatus => match (args.id.as_deref(), args.status) {
                (Some(_), Some(PlanStatus::Active)) => ToolOutput::error(
                    "a plan can only become active through explicit user approval",
                ),
                (Some(id), Some(status)) => {
                    match self.plans.set_status(session, id, status).await {
                        Ok(()) => {
                            plan_output(&self.plans, session, id, "Plan status updated.").await
                        }
                        Err(error) => ToolOutput::error(error),
                    }
                }
                (None, _) => ToolOutput::error("`set_status` needs an `id`"),
                (_, None) => ToolOutput::error("`set_status` needs a `status`"),
            },
            PlanAction::Delete => match args.id.as_deref() {
                Some(id) => match self.plans.delete(session, id).await {
                    Ok(true) => ToolOutput::text(format!("Plan `{id}` deleted.")),
                    Ok(false) => ToolOutput::error(format!("no plan `{id}` in this session")),
                    Err(error) => ToolOutput::error(format!("could not delete the plan: {error}")),
                },
                None => ToolOutput::error("`delete` needs an `id`"),
            },
        })
    }
}

async fn plan_output(
    plans: &PlanRepository,
    session: SessionId,
    id: &str,
    message: &str,
) -> ToolOutput {
    match plans.get(session, id).await {
        Ok(Some(plan)) => {
            ToolOutput::text(format!("{message}\nPlan `{id}`:\n{}", plan_markdown(&plan)))
        }
        Ok(None) => ToolOutput::error(format!(
            "{message} However, plan `{id}` could not be found afterward."
        )),
        Err(error) => ToolOutput::error(format!(
            "{message} However, the saved plan could not be read: {error}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use norte_code_protocol::Phase;
    use norte_code_protocol::sqlite::{InMemoryDirectory, SessionDirectory};

    fn stored_session(session: SessionId, key: &str) -> StoredSession {
        StoredSession {
            key: key.to_owned(),
            north: session,
            title: None,
            provider: "test".to_owned(),
            model: "test-model".to_owned(),
            created_at: chrono::Utc::now(),
            phase: Phase::Idle,
            stats: serde_json::json!({}),
            denied_tools: Vec::new(),
            allowed_tools: Vec::new(),
            compacted_from: None,
            workdir: None,
        }
    }

    fn temp_root() -> PathBuf {
        std::env::temp_dir().join(format!("norte-code-plans-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn markdown_round_trip_preserves_content_and_status() {
        let plan = Plan {
            id: "plan-1".to_owned(),
            content: "# Plan\n\nInspect, implement, test.\n".to_owned(),
            status: PlanStatus::Pending,
            created_at: Some(Utc::now()),
            updated_at: Some(Utc::now()),
            provider: Some("test".to_owned()),
            model: Some("model/with:punctuation".to_owned()),
        };
        let document = plan_markdown(&plan);
        assert_eq!(parse_plan(&plan.id, &document).expect("valid plan"), plan);
        assert!(plan_front_matter(&plan).contains("title: \"Plan\""));
        assert!(plan_front_matter(&plan).contains("created_at: "));
        assert!(plan_front_matter(&plan).contains("provider: \"test\""));
        assert!(plan_front_matter(&plan).contains("model: \"model/with:punctuation\""));
    }

    #[test]
    fn title_metadata_comes_from_the_first_markdown_line() {
        assert_eq!(
            plan_title("### A plan title\n\nDetails"),
            Some("A plan title")
        );
        assert_eq!(
            plan_title("A plain title\n\nDetails"),
            Some("A plain title")
        );
        assert_eq!(plan_title("\n# Title"), None);
        assert_eq!(plan_title("###"), None);
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn plans_are_markdown_files_and_survive_repository_reload() {
        let directory = Arc::new(InMemoryDirectory::new());
        let session = SessionId::new();
        let logical_id = "session-key";
        directory
            .save_session(&stored_session(session, logical_id))
            .await
            .expect("save session");
        let root = temp_root();
        let plans = PlanRepository::with_root(directory.clone(), root.clone());
        plans
            .ensure_session_folder(&SessionKey::new(logical_id))
            .await
            .expect("create session artifact folder");
        let plan = Plan {
            id: "plan-1".to_owned(),
            content: "# Inspect\n\nImplement and test.".to_owned(),
            status: PlanStatus::Pending,
            created_at: None,
            updated_at: None,
            provider: None,
            model: None,
        };
        plans.add(session, &plan).await.expect("save plan");

        let path = root.join(logical_id).join("plans").join("plan-1.md");
        let saved_markdown = tokio::fs::read_to_string(&path)
            .await
            .expect("read Markdown plan");
        assert!(saved_markdown.contains("# Inspect"));
        let metadata = parse_plan("plan-1", &saved_markdown).expect("plan metadata");
        assert!(metadata.created_at.is_some());
        assert!(metadata.updated_at.is_some());
        assert_eq!(metadata.provider.as_deref(), Some("test"));
        assert_eq!(metadata.model.as_deref(), Some("test-model"));
        let created_at = metadata.created_at.expect("creation timestamp");
        plans
            .set_status(session, "plan-1", PlanStatus::Active)
            .await
            .expect("update plan status");
        let updated = plans
            .get(session, "plan-1")
            .await
            .expect("read updated plan")
            .expect("plan exists");
        assert_eq!(updated.created_at, Some(created_at));
        assert!(updated.updated_at.expect("modification timestamp") >= created_at);
        assert_eq!(updated.provider.as_deref(), Some("test"));
        assert_eq!(updated.model.as_deref(), Some("test-model"));
        assert!(
            !directory.sessions().await.expect("list sessions")[0]
                .stats
                .to_string()
                .contains("plan_state")
        );

        tokio::fs::write(
            &path,
            "---\nstatus: pending\n---\n# User-edited plan\n\nEdited outside Norte Code.",
        )
        .await
        .expect("edit Markdown plan");
        let restarted = PlanRepository::with_root(directory, root.clone());
        let edited = restarted
            .get(session, "plan-1")
            .await
            .expect("read plan")
            .expect("plan exists");
        assert_eq!(
            edited.content,
            "# User-edited plan\n\nEdited outside Norte Code."
        );
        restarted
            .set_status(session, "plan-1", PlanStatus::Active)
            .await
            .expect("persist status");
        assert_eq!(
            restarted
                .get(session, "plan-1")
                .await
                .expect("read status")
                .unwrap()
                .status,
            PlanStatus::Active
        );
        assert_eq!(
            restarted
                .get(session, "plan-1")
                .await
                .expect("read edited body")
                .unwrap()
                .content,
            edited.content
        );
        assert!(
            restarted
                .delete(session, "plan-1")
                .await
                .expect("delete plan")
        );
        assert_eq!(restarted.list(session).await.expect("list plans").len(), 0);

        tokio::fs::remove_dir_all(root.join(logical_id))
            .await
            .expect("simulate removed session folder");
        restarted
            .ensure_session_folder(&SessionKey::new(logical_id))
            .await
            .expect("recreate folder when resuming session");
        assert!(root.join(logical_id).is_dir());
        tokio::fs::remove_dir_all(root)
            .await
            .expect("remove test files");
    }
}
