use super::*;
use norte::{CallId, Message};
use norte_code_wire::Phase as NortePhase;

#[tokio::test]
async fn costs_round_trip_and_replace_by_request_index() {
    use crate::costs::CostRecord;

    let store = SqliteStore::in_memory().expect("open");
    let record = CostRecord {
        session: "session".to_owned(),
        turn: 2,
        index: 7,
        model: "provider/model".to_owned(),
        input_tokens: Some(100),
        output_tokens: Some(20),
        reasoning_tokens: Some(5),
        cache_read_tokens: None,
        cache_write_tokens: None,
        cost_usd: Some(0.0125),
        recorded_at: Utc::now(),
    };
    store.record_cost(&record).await.expect("record");
    let mut replacement = record.clone();
    replacement.cost_usd = None;
    replacement.output_tokens = Some(25);
    store.record_cost(&replacement).await.expect("replace");
    let costs = store.costs(Some("session")).await.expect("costs");
    assert_eq!(costs.len(), 1);
    assert_eq!(costs[0].output_tokens, Some(25));
    assert!(costs[0].cost_usd.is_none());
    assert!(
        store.costs(Some("other")).await.expect("costs").is_empty(),
        "another session reports no cost"
    );
}

#[tokio::test]
async fn request_rows_keep_the_sandbox_flags() {
    let store = SqliteStore::in_memory().expect("open");
    let record = RequestRecord {
        id: "request".to_owned(),
        session: "session".to_owned(),
        north_session: SessionId::new().to_string(),
        turn: 1,
        provider: "provider".to_owned(),
        model: "model".to_owned(),
        endpoint: Some("https://example.invalid/v1".to_owned()),
        started_at: Utc::now(),
        duration_ms: 10,
        first_token_ms: Some(1),
        last_token_ms: Some(9),
        input_tokens: Some(10),
        output_tokens: Some(2),
        reasoning_tokens: None,
        cache_read_tokens: None,
        cache_write_tokens: None,
        estimated_input_tokens: None,
        cache_hit: false,
        tool_calls: vec!["command".to_owned()],
        reasoning: None,
        sandbox: true,
        sandbox_network: true,
        params: serde_json::Value::Null,
        extensions: serde_json::Value::Null,
        metadata: serde_json::Value::Null,
        cost_usd: None,
        finish_reason: None,
        status: RequestStatus::Ok,
        error: None,
        error_kind: None,
        retry_after_ms: None,
    };
    store.record_request(&record).await.expect("record");
    let rows = store.requests(Some("session")).await.expect("requests");
    assert_eq!(rows.len(), 1);
    assert!(rows[0].sandbox);
    assert!(rows[0].sandbox_network);
    assert_eq!(rows[0].tool_calls, ["command"]);
}

#[tokio::test]
async fn interrupted_streaming_sessions_become_idle() {
    let store = SqliteStore::in_memory().expect("open");
    store
        .save_session(&StoredSession {
            key: "session".to_owned(),
            north: SessionId::new(),
            title: None,
            provider: "provider".to_owned(),
            model: "model".to_owned(),
            created_at: Utc::now(),
            phase: Phase::Streaming,
            stats: serde_json::json!({}),
            denied_tools: Vec::new(),
            allowed_tools: Vec::new(),
            compacted_from: None,
            workdir: None,
        })
        .await
        .expect("save");
    assert_eq!(store.repair_interrupted().await.expect("repair"), 1);
    let sessions = store.sessions().await.expect("sessions");
    assert_eq!(sessions[0].phase, Phase::Idle);
    assert_eq!(store.repair_interrupted().await.expect("repair"), 0);
}

#[tokio::test]
async fn the_effect_journal_tracks_intents_until_they_finish() {
    let store = SqliteStore::in_memory().expect("open");
    let session = SessionId::new();
    let call = CallId::new();
    store
        .effect_started(session, call, "write", "epoch-one")
        .expect("intent");
    let pending = store.unfinished_effects().await.expect("pending");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].session, session);
    assert_eq!(pending[0].call_id, call);
    assert_eq!(pending[0].tool, "write");

    store
        .effect_finished(session, call, false)
        .expect("finished");
    assert!(
        store
            .unfinished_effects()
            .await
            .expect("pending")
            .is_empty(),
        "a finished effect is no longer pending"
    );
}

#[tokio::test]
async fn a_message_with_an_image_is_stored_and_comes_back() {
    // The node's message is written to a JSON column, so a media part has to
    // survive `serde_json`. It did not: an internally tagged `MediaSource`
    // cannot serialize a `Blob`, so the commit failed with "cannot serialize
    // tagged newtype variant" and the message that carried a pasted image was
    // never stored. This is the store that does the encoding, so this is
    // where the failure was visible.
    use norte::{MediaKind, MediaPart, MediaSource, MimeType};

    let store = SqliteStore::in_memory().expect("open");
    let session_id = SessionId::new();
    store
        .commit(Commit {
            session_id,
            expected_version: None,
            nodes: Vec::new(),
            head: None,
            phase: NortePhase::Idle,
            turn: 0,
            metadata: None,
        })
        .await
        .expect("create");

    let mime = MimeType::new("image/png").expect("mime");
    let blob = store.put(b"\x89PNG\r\n\x1a\n").await.expect("blob");
    let message = Message::new(
        norte::Role::User,
        vec![
            norte::Part::text("what is this?"),
            norte::Part::Media(MediaPart::new(
                MediaKind::Image,
                MediaSource::Blob(blob),
                mime,
            )),
        ],
    );
    let node = Node::message(message.clone(), None);
    store
        .commit(Commit {
            session_id,
            expected_version: Some(1),
            nodes: vec![node.clone()],
            head: Some(node.id),
            phase: NortePhase::Idle,
            turn: 1,
            metadata: None,
        })
        .await
        .expect("a node carrying an image is committed");

    let read_back = store.node(node.id).await.expect("node");
    assert_eq!(read_back.message, message, "the media part came back whole");
}

#[tokio::test]
async fn reopens_the_same_database() {
    let directory = std::env::temp_dir().join(format!(
        "norte-code-sqlite-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&directory).expect("directory");
    let path = directory.join("sessions.db");

    let session_id = SessionId::new();
    {
        let store = SqliteStore::open(&path).expect("open");
        store
            .commit(Commit {
                session_id,
                expected_version: None,
                nodes: Vec::new(),
                head: None,
                phase: NortePhase::Idle,
                turn: 0,
                metadata: None,
            })
            .await
            .expect("create");
        let node = Node::message(Message::user("persisted"), None);
        store
            .commit(Commit {
                session_id,
                expected_version: Some(1),
                nodes: vec![node.clone()],
                head: Some(node.id),
                phase: NortePhase::Idle,
                turn: 1,
                metadata: None,
            })
            .await
            .expect("append");
    }

    let store = SqliteStore::open(&path).expect("reopen");
    let session = store.load(session_id).await.expect("load");
    assert_eq!(session.version, 2);
    let history = store.history(session.head).await.expect("history");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].text(), "persisted");

    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn the_directory_round_trips_sessions_and_questions() {
    let store = SqliteStore::in_memory().expect("open");
    let session = StoredSession {
        key: "abc".to_owned(),
        north: SessionId::new(),
        title: Some("first".to_owned()),
        provider: "openai".to_owned(),
        model: "gpt".to_owned(),
        created_at: Utc::now(),
        phase: Phase::Idle,
        stats: serde_json::json!({"cost": 0.5}),
        denied_tools: vec!["command".to_owned()],
        allowed_tools: vec!["write".to_owned()],
        compacted_from: None,
        workdir: Some("/home/someone/src/thing".to_owned()),
    };
    store.save_session(&session).await.expect("save");
    assert_eq!(store.sessions().await.expect("list"), vec![session.clone()]);

    let question = StoredQuestion {
        id: "q0".to_owned(),
        session: session.north,
        call_id: "call-1".to_owned(),
        question: "Which file?".to_owned(),
        options: vec!["src/lib.rs".to_owned(), "src/main.rs".to_owned()],
        multiple: false,
        answer: None,
    };
    store.save_question(&question).await.expect("question");
    assert!(
        store
            .answer_question("call-1", "src/lib.rs")
            .await
            .expect("answer")
    );
    assert!(
        !store
            .answer_question("call-1", "again")
            .await
            .expect("answer")
    );
    let loaded = store
        .question("call-1")
        .await
        .expect("load question")
        .expect("present");
    assert_eq!(loaded.answer.as_deref(), Some("src/lib.rs"));

    store.delete_session("abc").await.expect("delete");
    assert!(
        store.sessions().await.expect("list").is_empty(),
        "nothing is left after the delete"
    );
}

/// Counts the rows the database holds for one session: everything a delete
/// has to reach. `heads` are the node ids the session's tree is made of,
/// which is how nodes are found: they carry no session of their own.
fn rows_for(
    store: &SqliteStore,
    key: &str,
    north: SessionId,
    heads: &[NodeId],
) -> Vec<(&'static str, i64)> {
    let connection = store.lock();
    let count = |sql: &str, value: &str| -> i64 {
        connection
            .query_row(sql, params![value], |row| row.get::<_, i64>(0))
            .expect("count")
    };
    let north = north.to_string();
    let mut nodes = 0;
    for head in heads {
        nodes += count(
            "SELECT count(*) FROM nodes WHERE id = ?1",
            &head.to_string(),
        );
    }
    vec![
        (
            "sessions",
            count("SELECT count(*) FROM sessions WHERE id = ?1", &north),
        ),
        ("nodes", nodes),
        (
            "costs",
            count("SELECT count(*) FROM costs WHERE session = ?1", key),
        ),
        (
            "requests",
            count("SELECT count(*) FROM requests WHERE session = ?1", key),
        ),
        (
            "effects",
            count("SELECT count(*) FROM effects WHERE session = ?1", &north),
        ),
        (
            "questions",
            count("SELECT count(*) FROM questions WHERE session = ?1", &north),
        ),
    ]
}

/// Seeds a session's logical row and a two-node conversation under it,
/// returning the node ids: the chain a delete walks back from the head.
async fn seed_conversation(store: &SqliteStore, key: &str, north: SessionId) -> Vec<NodeId> {
    store
        .save_session(&StoredSession {
            key: key.to_owned(),
            north,
            title: Some(key.to_owned()),
            provider: "openai".to_owned(),
            model: "gpt".to_owned(),
            created_at: Utc::now(),
            phase: Phase::Idle,
            stats: serde_json::json!({}),
            denied_tools: Vec::new(),
            allowed_tools: Vec::new(),
            compacted_from: None,
            workdir: None,
        })
        .await
        .expect("save");

    store
        .commit(Commit {
            session_id: north,
            expected_version: None,
            nodes: Vec::new(),
            head: None,
            phase: NortePhase::Idle,
            turn: 0,
            metadata: None,
        })
        .await
        .expect("create");
    let first = Node::message(Message::user("hello"), None);
    store
        .commit(Commit {
            session_id: north,
            expected_version: Some(1),
            nodes: vec![first.clone()],
            head: Some(first.id),
            phase: NortePhase::Idle,
            turn: 1,
            metadata: None,
        })
        .await
        .expect("append");
    let second = Node::message(Message::assistant("hi"), Some(first.id));
    store
        .commit(Commit {
            session_id: north,
            expected_version: Some(2),
            nodes: vec![second.clone()],
            head: Some(second.id),
            phase: NortePhase::Idle,
            turn: 2,
            metadata: None,
        })
        .await
        .expect("append");
    vec![first.id, second.id]
}

/// Seeds one row in each table that outlives a session: the ledger, the
/// telemetry row, the effect and the question. Some keyed by the logical
/// key and some by the session id, which is the point.
async fn seed_rows(store: &SqliteStore, key: &str, north: SessionId) {
    use crate::costs::CostRecord;

    store
        .record_cost(&CostRecord {
            session: key.to_owned(),
            turn: 1,
            index: 0,
            model: "openai/gpt".to_owned(),
            input_tokens: Some(1),
            output_tokens: Some(1),
            reasoning_tokens: None,
            cache_read_tokens: None,
            cache_write_tokens: None,
            cost_usd: Some(0.01),
            recorded_at: Utc::now(),
        })
        .await
        .expect("cost");
    store
        .record_request(&RequestRecord {
            id: format!("request-{key}"),
            session: key.to_owned(),
            north_session: north.to_string(),
            turn: 1,
            provider: "provider".to_owned(),
            model: "model".to_owned(),
            endpoint: None,
            started_at: Utc::now(),
            duration_ms: 10,
            first_token_ms: Some(1),
            last_token_ms: Some(9),
            input_tokens: Some(10),
            output_tokens: Some(2),
            reasoning_tokens: None,
            cache_read_tokens: None,
            cache_write_tokens: None,
            estimated_input_tokens: None,
            cache_hit: false,
            tool_calls: Vec::new(),
            reasoning: None,
            sandbox: false,
            sandbox_network: false,
            params: serde_json::Value::Null,
            extensions: serde_json::Value::Null,
            metadata: serde_json::Value::Null,
            cost_usd: None,
            finish_reason: None,
            status: RequestStatus::Ok,
            error: None,
            error_kind: None,
            retry_after_ms: None,
        })
        .await
        .expect("request");
    store
        .effect_started(north, CallId::new(), "write", "epoch")
        .expect("effect");
    store
        .save_question(&StoredQuestion {
            id: format!("q-{key}"),
            session: north,
            call_id: format!("call-{key}"),
            question: "which file?".to_owned(),
            options: Vec::new(),
            multiple: false,
            answer: None,
        })
        .await
        .expect("question");
}

#[tokio::test]
async fn deleting_a_session_takes_every_row_it_left_behind() {
    // A delete that cleared only `logical_sessions` left the Norte session,
    // its node tree and its cost, telemetry, effect and question rows
    // behind: a session that vanished from the list but kept growing the
    // database. The key names the logical row, the id names the rest, and
    // both spellings live in the `session` column of different tables, so
    // this seeds a row for each and checks that all of them are gone.
    let store = SqliteStore::in_memory().expect("open");
    let north = SessionId::new();
    let nodes = seed_conversation(&store, "doomed", north).await;
    seed_rows(&store, "doomed", north).await;
    let before = rows_for(&store, "doomed", north, &nodes);
    assert!(
        before.iter().all(|(_, count)| *count > 0),
        "every table was seeded: {before:?}"
    );

    // A second session, so the delete can be shown to be scoped: a delete
    // that emptied the tables outright would pass the checks above too.
    let keeper = SessionId::new();
    let keeper_nodes = seed_conversation(&store, "keeper", keeper).await;
    seed_rows(&store, "keeper", keeper).await;
    let before_keeper = rows_for(&store, "keeper", keeper, &keeper_nodes);

    store.delete_session("doomed").await.expect("delete");

    let gone = store.sessions().await.expect("list");
    assert!(
        gone.iter().all(|session| session.key != "doomed"),
        "the deleted session is not in the list"
    );
    let after = rows_for(&store, "doomed", north, &nodes);
    assert!(
        after.iter().all(|(_, count)| *count == 0),
        "no table kept a row for it: {after:?}"
    );
    assert_eq!(
        rows_for(&store, "keeper", keeper, &keeper_nodes),
        before_keeper,
        "the other session is untouched"
    );
}

#[tokio::test]
async fn a_session_stored_before_the_directory_field_still_loads() {
    // The field is in the blob and not in a column, which is what makes it
    // optional: a row written before it existed decodes to `None` and the
    // session opens wherever the command was typed, which is what `resume`
    // did before there was anything to remember.
    let store = SqliteStore::in_memory().expect("open");
    let older = StoredSession {
        key: "older".to_owned(),
        north: SessionId::new(),
        title: None,
        provider: "openai".to_owned(),
        model: "gpt".to_owned(),
        created_at: Utc::now(),
        phase: Phase::Idle,
        stats: serde_json::json!({}),
        denied_tools: Vec::new(),
        allowed_tools: Vec::new(),
        compacted_from: None,
        workdir: None,
    };
    store.save_session(&older).await.expect("save");
    let loaded = store.sessions().await.expect("list");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].workdir, None, "no directory was ever recorded");
    assert_eq!(loaded[0].key, "older");
}

#[tokio::test]
async fn a_session_stored_in_a_legacy_grant_envelope_still_loads() {
    let store = SqliteStore::in_memory().expect("open");
    let id = SessionId::new();
    let raw = format!(
        r#"{{"session":{{"id":"{id}","phase":"idle","version":1,"turn":0,"created_at":"2026-10-30T00:00:00Z","updated_at":"2026-10-30T00:00:00Z"}},"grants":[]}}"#
    );
    store
        .run(move |conn| {
            conn.execute(
                "INSERT INTO sessions (id, version, data) VALUES (?1, 1, ?2)",
                params![id.to_string(), raw],
            )
            .map_err(Fault::from)
        })
        .await
        .expect("insert");
    let session = store.load(id).await.expect("load");
    assert_eq!(session.id, id);
    assert_eq!(session.version, 1);
}
