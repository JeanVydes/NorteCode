use super::*;

fn row(key: &str, title: Option<&str>, phase: Phase, unfinished: bool) -> SessionRow {
    SessionRow {
        key: key.to_owned(),
        title: title.map(str::to_owned),
        provider: "river".to_owned(),
        model: "deepseek".to_owned(),
        phase,
        created_at: chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
        updated_at: chrono::DateTime::parse_from_rfc3339("2026-01-02T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
        context_used: 0,
        unfinished,
    }
}

fn stored(phase: &str) -> StoredSession {
    StoredSession {
        key: "aaaaaaaa-0000-0000-0000-000000000000".to_owned(),
        north: norte::SessionId::new(),
        title: None,
        provider: "openai".to_owned(),
        model: "gpt-5".to_owned(),
        created_at: chrono::Utc::now(),
        phase: match phase {
            "streaming" => Phase::Streaming,
            "awaiting_tool" => Phase::AwaitingTool,
            _ => Phase::Idle,
        },
        stats: serde_json::Value::Null,
        denied_tools: Vec::new(),
        allowed_tools: Vec::new(),
        compacted_from: None,
        workdir: None,
    }
}

#[test]
fn the_menu_offers_the_new_session_the_last_one_and_the_saved_ones() {
    let rows = vec![
        row("7c19ab22-0000", None, Phase::Streaming, true),
        row("03fe0fad-a2ee", Some("browser work"), Phase::Idle, false),
    ];
    let items = session_items(&rows);
    // Two fixed entries and one row per session, in the order the rows
    // arrived in, which is newest first.
    assert_eq!(items.len(), 4);
    assert_eq!(items[0].label, "New session");
    assert!(items[0].fixed, "the fixed entries are drawn bold");
    assert_eq!(items[1].label, "Last session");
    // The state of the newest session is on the row it describes, rather
    // than only being reachable by opening it.
    assert_eq!(items[1].note.as_deref(), Some("unfinished"));
    // The newest session is the running one; the one after it is idle, and
    // an idle session says nothing: "ready" on every row is a column of
    // noise.
    assert!(items[2].label.starts_with("7c19ab22"), "{}", items[2].label);
    assert_eq!(items[2].note.as_deref(), Some("unfinished"));
    // A titled session leads with the title and keeps its key beside it, so
    // both what a person calls it and what they would type are on the row.
    assert!(
        items[3].label.starts_with("browser work"),
        "{}",
        items[3].label
    );
    assert_eq!(items[3].note, None);
    assert!(
        items[3].search.contains("03fe0fad-a2ee"),
        "the whole key is searchable, not just the eight shown: {}",
        items[3].search
    );
}

#[test]
fn every_session_reaches_the_menu_so_the_filter_can_find_it() {
    // The menu draws a window and scrolls it, so a long list is cheap to
    // show; what it cannot do is search a row it was never given. The list
    // was cut to a fixed ten here, so typing the key of the eleventh session
    // matched nothing and the session looked absent though it existed. Every
    // row is handed over now, newest first.
    let rows: Vec<SessionRow> = (0..30)
        .map(|index| row(&format!("{index:08x}-0000"), None, Phase::Idle, false))
        .collect();
    let items = session_items(&rows);
    assert_eq!(
        items.len(),
        rows.len() + 2,
        "two fixed entries and every row"
    );
    // The last one is a row of the menu, and it is searchable by its key.
    let last = &items[items.len() - 1];
    assert!(last.label.starts_with("0000001d"), "{}", last.label);
    assert!(
        last.search.contains("0000001d-0000"),
        "the whole key is searchable: {}",
        last.search
    );
}

#[test]
fn a_row_is_named_by_its_title_or_its_short_key() {
    let named = row("03fe0fad-a2ee", Some("browser work"), Phase::Idle, false);
    assert!(
        named.label().starts_with("browser work"),
        "{}",
        named.label()
    );
    assert_eq!(named.state(), None);
    let untitled = row("03fe0fad-a2ee", None, Phase::Idle, false);
    assert!(
        untitled.label().starts_with("03fe0fad"),
        "{}",
        untitled.label()
    );
    // A blank title is not a name: the key is all it has.
    let blank = row("03fe0fad-a2ee", Some("   "), Phase::Idle, false);
    assert!(blank.label().starts_with("03fe0fad"), "{}", blank.label());
}

#[test]
fn a_session_with_something_to_continue_says_so() {
    let unfinished = row("aaaaaaaa", None, Phase::Streaming, true);
    assert_eq!(unfinished.state(), Some(SessionState::Unfinished));
    let stale = row("aaaaaaaa", None, Phase::AwaitingTool, false);
    assert_eq!(stale.state(), Some(SessionState::Waiting));
}

#[test]
fn a_stored_phase_becomes_the_phase_the_client_shows() {
    assert_eq!(
        row_of(stored("streaming"), chrono::Utc::now()).phase,
        Phase::Streaming
    );
    assert_eq!(
        row_of(stored("awaiting_tool"), chrono::Utc::now()).phase,
        Phase::AwaitingTool
    );
    // Anything else starts idle, which is what the service does with a phase
    // it does not recognise.
    assert_eq!(
        row_of(stored("idle"), chrono::Utc::now()).phase,
        Phase::Idle
    );
    assert_eq!(
        row_of(stored("nonsense"), chrono::Utc::now()).phase,
        Phase::Idle
    );
    // A turn in flight outlived its process, so there is work to continue; a
    // tool that was about to run is not the same thing.
    assert!(row_of(stored("streaming"), chrono::Utc::now()).unfinished);
    assert!(!row_of(stored("idle"), chrono::Utc::now()).unfinished);
    assert!(!row_of(stored("awaiting_tool"), chrono::Utc::now()).unfinished);
}

#[test]
fn the_choice_is_returned_without_writing_server_configuration() {
    let mut config = Config::default();
    let choice = remember(
        &mut config,
        None,
        "river",
        "deepseek-ai/DeepSeek-V4.1-Flash",
        Style::plain(),
    );
    // The pair is carried apart: the provider is its own key and the model
    // keeps its slash, so neither is ever taken from a joined string.
    assert_eq!(choice.provider, "river");
    assert_eq!(choice.model, "deepseek-ai/DeepSeek-V4.1-Flash");
    assert!(config.default_provider.is_none());
    assert!(config.default_model.is_none());
}
