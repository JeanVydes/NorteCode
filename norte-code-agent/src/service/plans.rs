//! Markdown-backed plans and the model's `plan` tool.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use norte::{FnExecutor, Result, SessionId, ToolContext, ToolOutput};
use norte_code_protocol::plans::{Plan, parse_plan, read_plan_header};
use norte_code_protocol::sqlite::{SessionDirectory, StoredSession};
use norte_code_wire::PlanStatus;
use norte_code_wire::{ServerEventKind, SessionKey};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::mapping::single;
use super::{EventStream, Service, ServiceError};

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

impl Service {
    /// The session's plans, newest first.
    pub(super) async fn list_plans_event(
        &self,
        request_id: u64,
        session: SessionKey,
    ) -> std::result::Result<EventStream, ServiceError> {
        let (nid, _, _) = self.require_record(&session)?;
        let mut plans = self
            .plans
            .list(nid)
            .await
            .map_err(ServiceError::Plan)?
            .into_iter()
            .map(|(plan, _)| norte_code_wire::PlanInfo {
                id: plan.id,
                content: plan.content,
                status: plan.status,
                title: plan.title,
                revision: plan.revision,
                model: plan.model,
            })
            .collect::<Vec<_>>();
        plans.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(single(
            request_id,
            Some(session),
            ServerEventKind::PlanList { plans },
        ))
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

    pub(super) async fn list(
        &self,
        session: SessionId,
    ) -> std::result::Result<Vec<(Plan, PathBuf)>, String> {
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
            let path = entry.path();
            plans.push((read_plan_header(path.clone(), &id).await?, path));
        }
        plans.sort_by(|left, right| {
            right
                .0
                .updated_at
                .cmp(&left.0.updated_at)
                .then_with(|| left.0.id.cmp(&right.0.id))
        });
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

    pub(super) async fn revise(
        &self,
        session: SessionId,
        id: &str,
        content: String,
    ) -> std::result::Result<Plan, String> {
        let _guard = self.writes.lock().await;
        let mut plan = self
            .get(session, id)
            .await?
            .ok_or_else(|| format!("no plan `{id}` in this session"))?;
        plan.content = content;
        plan.title = plan_title(&plan.content).map(str::to_owned);
        plan.status = PlanStatus::Pending;
        plan.revision = plan.revision.saturating_add(1);
        plan.updated_at = Some(Utc::now());
        self.write(session, &plan).await?;
        Ok(plan)
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
    writeln!(front_matter, "revision: {}", plan.revision).expect("writing to a string cannot fail");
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

/// Operations exposed by the model's `plan` tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename_all = "snake_case")]
enum PlanAction {
    /// Submit a plan for explicit user approval.
    Propose,
    /// Replace an existing plan and submit the revision for approval.
    Revise,
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

    async fn review(
        &self,
        context: &ToolContext,
        session: SessionId,
        id: &str,
        plan_content: &str,
    ) -> ToolOutput {
        let answer = self
            .questions
            .ask(
                context,
                format!("NORTE_CODE_PLAN_REVIEW\n{plan_content}"),
                vec!["Approve".to_owned(), "Reject".to_owned()],
                false,
            )
            .await;
        match answer.as_deref() {
            Ok("Approve") => match self.plans.set_status(session, id, PlanStatus::Active).await {
                Ok(()) => {
                    plan_output(&self.plans, session, id, "Plan approved and activated.").await
                }
                Err(error) => ToolOutput::error(format!("could not approve the plan: {error}")),
            },
            Ok("Reject") => match self
                .plans
                .set_status(session, id, PlanStatus::Canceled)
                .await
            {
                Ok(()) => plan_output(&self.plans, session, id, "Plan rejected.").await,
                Err(error) => ToolOutput::error(format!("could not reject the plan: {error}")),
            },
            Ok(other) => ToolOutput::error(format!(
                "unexpected plan approval response `{other}`; the plan remains pending"
            )),
            Err(error) => ToolOutput::error(format!(
                "could not get approval: {error}; the plan remains pending"
            )),
        }
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
                    let plan =
                        Plan::pending(uuid::Uuid::new_v4().to_string(), content.trim().to_owned());
                    if let Err(error) = self.plans.add(session, &plan).await {
                        ToolOutput::error(format!("could not save the plan: {error}"))
                    } else {
                        self.review(context, session, &plan.id, &plan.content).await
                    }
                } else {
                    ToolOutput::error("`propose` needs non-empty `content`")
                }
            }
            PlanAction::Revise => match (args.id.as_deref(), args.content) {
                (Some(id), Some(revised_content)) if !revised_content.trim().is_empty() => {
                    match self
                        .plans
                        .revise(session, id, revised_content.trim().to_owned())
                        .await
                    {
                        Ok(plan) => self.review(context, session, &plan.id, &plan.content).await,
                        Err(error) => {
                            ToolOutput::error(format!("could not revise the plan: {error}"))
                        }
                    }
                }
                (None, _) => ToolOutput::error("`revise` needs an `id`"),
                (_, None | Some(_)) => ToolOutput::error("`revise` needs non-empty `content`"),
            },
            PlanAction::List => match self.plans.list(session).await {
                Ok(plans) if plans.is_empty() => ToolOutput::text("No plans in this session."),
                Ok(plans) => ToolOutput::text(
                    plans
                        .iter()
                        .map(|(plan, path)| {
                            format!(
                                "{}\n{}\npath: {}",
                                plan.id,
                                plan_front_matter(plan),
                                path.display()
                            )
                        })
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
    use norte_code_protocol::sqlite::{InMemoryDirectory, SessionDirectory};
    use norte_code_wire::Phase;

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
        let mut plan = Plan::pending("plan-1", "# Plan\n\nInspect, implement, test.\n");
        plan.created_at = Some(Utc::now());
        plan.updated_at = Some(Utc::now());
        plan.provider = Some("test".to_owned());
        plan.model = Some("model/with:punctuation".to_owned());
        plan.title = Some("Plan".to_owned());
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
        let plan = Plan::pending("plan-1", "# Inspect\n\nImplement and test.");
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
        assert_eq!(metadata.revision, 0);
        let created_at = metadata.created_at.expect("creation timestamp");
        let revised = plans
            .revise(
                session,
                "plan-1",
                "# Revised\n\nImplement the updated approach.".to_owned(),
            )
            .await
            .expect("revise plan");
        assert_eq!(revised.revision, 1);
        assert_eq!(revised.status, PlanStatus::Pending);
        assert_eq!(revised.created_at, Some(created_at));
        assert_eq!(revised.provider.as_deref(), Some("test"));
        assert_eq!(revised.model.as_deref(), Some("test-model"));
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
