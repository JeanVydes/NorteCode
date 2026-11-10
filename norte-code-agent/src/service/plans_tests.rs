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
