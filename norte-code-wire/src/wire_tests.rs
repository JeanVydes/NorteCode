use super::*;
use serde_json::json;

fn test_request(kind: ClientRequestKind) -> ClientRequest {
    ClientRequest {
        request_id: 7,
        kind,
    }
}

fn round_trips_request(kind: ClientRequestKind) -> ClientRequest {
    let request = test_request(kind);
    let json = serde_json::to_string(&request).expect("serialize a request");
    serde_json::from_str(&json).expect("a request reads back what was written")
}

#[test]
fn a_fork_without_new_fields_adds_no_fields() {
    let request = round_trips_request(ClientRequestKind::ForkSession {
        session: SessionKey::new("s"),
        title: None,
        from_turn_id: None,
        inclusive: None,
        carry_usage: None,
        agent_id: None,
        include: None,
    });
    let json = serde_json::to_string(&request).expect("serialize a fork");
    assert!(!json.contains("agent_id"));
    assert!(!json.contains("include"));
}

#[test]
fn a_fork_with_include_round_trips() {
    let request = round_trips_request(ClientRequestKind::ForkSession {
        session: SessionKey::new("s"),
        title: None,
        from_turn_id: None,
        inclusive: None,
        carry_usage: None,
        agent_id: Some("reviewer".to_owned()),
        include: Some(ForkInclude {
            uploads: true,
            outputs: false,
            workspace: true,
        }),
    });
    match request.kind {
        ClientRequestKind::ForkSession {
            agent_id, include, ..
        } => {
            assert_eq!(agent_id.as_deref(), Some("reviewer"));
            assert_eq!(
                include,
                Some(ForkInclude {
                    uploads: true,
                    outputs: false,
                    workspace: true,
                })
            );
        }
        other => panic!("expected a fork, got {other:?}"),
    }
}

#[test]
fn an_include_defaults_every_flag_to_false() {
    let include: ForkInclude =
        serde_json::from_str("{}").expect("an empty include reads as all false");
    assert_eq!(
        include,
        ForkInclude {
            uploads: false,
            outputs: false,
            workspace: false,
        }
    );
    let json = serde_json::to_string(&include).expect("serialize an include");
    assert_eq!(
        json,
        r#"{"uploads":false,"outputs":false,"workspace":false}"#
    );
}

#[test]
fn an_include_rejects_an_unknown_flag() {
    let error = serde_json::from_str::<ForkInclude>(r#"{"uploads":true,"children":true}"#)
        .expect_err("an unknown include flag must be rejected");
    assert!(
        error.to_string().contains("unknown field"),
        "unexpected error: {error}"
    );
}

#[test]
fn a_session_compact_round_trips_snake_case() {
    let request = round_trips_request(ClientRequestKind::SessionCompact {
        session: SessionKey::new("s"),
        strategy: CompactStrategy::Summarize,
        keep_last_turns: Some(2),
        instructions: Some("keep the code".to_owned()),
        trigger: CompactTrigger::Manual,
    });
    let json = serde_json::to_string(&request).expect("serialize a compact");
    assert!(json.contains(r#""type":"session_compact""#));
    assert!(json.contains(r#""strategy":"summarize""#));
    assert!(json.contains(r#""trigger":"manual""#));
    match request.kind {
        ClientRequestKind::SessionCompact {
            strategy,
            keep_last_turns,
            instructions,
            trigger,
            ..
        } => {
            assert_eq!(strategy, CompactStrategy::Summarize);
            assert_eq!(keep_last_turns, Some(2));
            assert_eq!(instructions.as_deref(), Some("keep the code"));
            assert_eq!(trigger, CompactTrigger::Manual);
        }
        other => panic!("expected a compact, got {other:?}"),
    }
}

#[test]
fn a_session_compact_defaults_to_manual() {
    let request: ClientRequest = serde_json::from_str(
        r#"{"request_id":1,"kind":{"type":"session_compact","session":"s","strategy":"truncate"}}"#,
    )
    .expect("a compact without a trigger reads");
    match request.kind {
        ClientRequestKind::SessionCompact {
            strategy, trigger, ..
        } => {
            assert_eq!(strategy, CompactStrategy::Truncate);
            assert_eq!(trigger, CompactTrigger::Manual);
        }
        other => panic!("expected a compact, got {other:?}"),
    }
}

#[test]
fn a_session_compact_rejects_an_unknown_field() {
    let error = serde_json::from_str::<ClientRequest>(
        r#"{"request_id":1,"kind":{"type":"session_compact","session":"s","strategy":"summarize","mode":"auto"}}"#,
    )
    .expect_err("an unknown compact field must be rejected");
    assert!(
        error.to_string().contains("unknown field"),
        "unexpected error: {error}"
    );
}

#[test]
fn a_context_pin_round_trips() {
    let request = round_trips_request(ClientRequestKind::ContextPin {
        session: SessionKey::new("s"),
        turn_ids: vec!["turn_1".to_owned(), "turn_3".to_owned()],
        pinned: true,
    });
    let json = serde_json::to_string(&request).expect("serialize a pin");
    assert!(json.contains(r#""type":"context_pin""#));
    assert!(json.contains(r#""turn_ids":["turn_1","turn_3"]"#));
    match request.kind {
        ClientRequestKind::ContextPin {
            turn_ids, pinned, ..
        } => {
            assert_eq!(turn_ids, ["turn_1", "turn_3"]);
            assert!(pinned);
        }
        other => panic!("expected a pin, got {other:?}"),
    }
}

#[test]
fn a_session_rewind_round_trips() {
    let request = round_trips_request(ClientRequestKind::SessionRewind {
        session: SessionKey::new("s"),
        to_turn_id: "turn_2".to_owned(),
        inclusive: true,
        cancel_pending: true,
        workspace: RewindWorkspace::Restore,
    });
    let json = serde_json::to_string(&request).expect("serialize a rewind");
    assert!(json.contains(r#""type":"session_rewind""#));
    assert!(json.contains(r#""workspace":"restore""#));
    match request.kind {
        ClientRequestKind::SessionRewind {
            to_turn_id,
            inclusive,
            cancel_pending,
            workspace,
            ..
        } => {
            assert_eq!(to_turn_id, "turn_2");
            assert!(inclusive);
            assert!(cancel_pending);
            assert_eq!(workspace, RewindWorkspace::Restore);
        }
        other => panic!("expected a rewind, got {other:?}"),
    }
}

#[test]
fn a_session_rewind_defaults_the_booleans() {
    let request: ClientRequest = serde_json::from_str(
        r#"{"request_id":1,"kind":{"type":"session_rewind","session":"s","to_turn_id":"turn_2","workspace":"keep"}}"#,
    )
    .expect("a rewind without the booleans reads");
    match request.kind {
        ClientRequestKind::SessionRewind {
            inclusive,
            cancel_pending,
            workspace,
            ..
        } => {
            assert!(!inclusive);
            assert!(!cancel_pending);
            assert_eq!(workspace, RewindWorkspace::Keep);
        }
        other => panic!("expected a rewind, got {other:?}"),
    }
}

#[test]
fn a_history_get_round_trips() {
    let request = round_trips_request(ClientRequestKind::HistoryGet {
        session: SessionKey::new("s"),
        before_turn_id: Some("turn_5".to_owned()),
        limit: 50,
        include_rewound: true,
    });
    let json = serde_json::to_string(&request).expect("serialize a history get");
    assert!(json.contains(r#""type":"history_get""#));
    assert!(json.contains(r#""before_turn_id":"turn_5""#));
    match request.kind {
        ClientRequestKind::HistoryGet {
            before_turn_id,
            limit,
            include_rewound,
            ..
        } => {
            assert_eq!(before_turn_id.as_deref(), Some("turn_5"));
            assert_eq!(limit, 50);
            assert!(include_rewound);
        }
        other => panic!("expected a history get, got {other:?}"),
    }
}

#[test]
fn a_history_page_event_round_trips() {
    let event = ServerEvent::new(
        1,
        Some(SessionKey::new("s")),
        ServerEventKind::HistoryPage {
            items: vec![HistoryItem {
                id: "1".to_owned(),
                kind: "user".to_owned(),
                content: "hello".to_owned(),
                created_at: 1_700_000_000_000,
                turn_id: Some("turn_1".to_owned()),
                role: HistorySpeaker::User,
                provenance: Some(json!({"origin": "cli"})),
            }],
            has_more: true,
        },
    );
    let json = serde_json::to_string(&event).expect("serialize a history page");
    assert!(json.contains(r#""event":"history_page""#));
    assert!(json.contains(r#""role":"user""#));
    let back: ServerEvent = serde_json::from_str(&json).expect("a history page reads back");
    match back.kind {
        ServerEventKind::HistoryPage { items, has_more } => {
            assert!(has_more);
            assert_eq!(items.len(), 1);
            let item = &items[0];
            assert_eq!(item.id, "1");
            assert_eq!(item.kind, "user");
            assert_eq!(item.content, "hello");
            assert_eq!(item.created_at, 1_700_000_000_000);
            assert_eq!(item.turn_id.as_deref(), Some("turn_1"));
            assert_eq!(item.role, HistorySpeaker::User);
            assert_eq!(item.provenance, Some(json!({"origin": "cli"})));
        }
        other => panic!("expected a history page, got {other:?}"),
    }
}

#[test]
fn a_history_item_rejects_an_unknown_field() {
    let error = serde_json::from_str::<HistoryItem>(
        r#"{"id":"1","kind":"user","content":"hi","created_at":0,"role":"user","extra":true}"#,
    )
    .expect_err("an unknown history field must be rejected");
    assert!(
        error.to_string().contains("unknown field"),
        "unexpected error: {error}"
    );
}

#[test]
fn a_send_message_without_new_fields_adds_no_fields() {
    let request = round_trips_request(ClientRequestKind::SendMessage {
        session: Some(SessionKey::new("abc")),
        text: "hi".to_owned(),
        attachments: Vec::new(),
        budget: None,
        reasoning: None,
        output: Vec::new(),
        response_format: None,
        metadata: None,
        replay: false,
    });
    let json = serde_json::to_string(&request).expect("serialize a message");
    assert!(!json.contains("response_format"));
    assert!(!json.contains("\"output\""));
    assert!(!json.contains("replaces_turn_id"));
    assert_eq!(request.request_id, 7);
}

#[test]
fn a_send_message_with_format_and_output_round_trips() {
    let request = round_trips_request(ClientRequestKind::SendMessage {
        session: Some(SessionKey::new("abc")),
        text: "hi".to_owned(),
        attachments: Vec::new(),
        budget: None,
        reasoning: None,
        output: vec![OutputModality::Text, OutputModality::Image],
        response_format: Some(ResponseFormat {
            kind: ResponseFormatType::JsonSchema,
            schema: Some(json!({"type": "object"})),
            strict: true,
        }),
        metadata: None,
        replay: false,
    });
    let json = serde_json::to_string(&request).expect("serialize a message");
    assert!(json.contains(r#""output":["text","image"]"#));
    assert!(json.contains(r#""response_format":{"type":"json_schema""#));
    match request.kind {
        ClientRequestKind::SendMessage {
            output,
            response_format,
            ..
        } => {
            assert_eq!(output, [OutputModality::Text, OutputModality::Image]);
            assert_eq!(
                response_format,
                Some(ResponseFormat {
                    kind: ResponseFormatType::JsonSchema,
                    schema: Some(json!({"type": "object"})),
                    strict: true,
                })
            );
        }
        other => panic!("expected a message, got {other:?}"),
    }
}

#[test]
fn a_response_format_defaults_strict_to_false() {
    let format: ResponseFormat =
        serde_json::from_str(r#"{"type":"text"}"#).expect("a text format reads");
    assert_eq!(
        format,
        ResponseFormat {
            kind: ResponseFormatType::Text,
            schema: None,
            strict: false,
        }
    );
}

#[test]
fn a_response_format_rejects_an_unknown_field() {
    let error = serde_json::from_str::<ResponseFormat>(
        r#"{"type":"text","strict":false,"verbosity":"high"}"#,
    )
    .expect_err("an unknown format field must be rejected");
    assert!(
        error.to_string().contains("unknown field"),
        "unexpected error: {error}"
    );
}

#[test]
fn a_pending_override_round_trips() {
    let pending = PendingOverride {
        model: Some(PendingModel {
            provider: "acme".to_owned(),
            model: "acme/big".to_owned(),
        }),
        reasoning: Some(Reasoning::Effort {
            effort: "high".to_owned(),
        }),
        budget: Some(Budget {
            unit: BudgetUnit::Usd,
            limit: 2.0,
            action: BudgetAction::Stop,
        }),
    };
    let json = serde_json::to_string(&pending).expect("serialize a pending override");
    assert!(json.contains(r#""provider":"acme""#));
    assert!(json.contains(r#""model":"acme/big""#));
    let back: PendingOverride = serde_json::from_str(&json).expect("a pending override reads back");
    assert_eq!(
        back.model,
        Some(PendingModel {
            provider: "acme".to_owned(),
            model: "acme/big".to_owned(),
        })
    );
    assert_eq!(
        back.reasoning,
        Some(Reasoning::Effort {
            effort: "high".to_owned(),
        })
    );
    assert!(back.budget.is_some());
    let budget = back.budget.expect("the budget reads back");
    assert_eq!(budget.unit, BudgetUnit::Usd);
    assert_eq!(budget.limit, 2.0);
    assert_eq!(budget.action, BudgetAction::Stop);
}

#[test]
fn an_empty_pending_override_adds_no_fields() {
    let pending = PendingOverride::default();
    let json = serde_json::to_string(&pending).expect("serialize an empty override");
    assert_eq!(json, "{}");
    let back: PendingOverride = serde_json::from_str("{}").expect("an empty override reads");
    assert_eq!(back.model, None);
    assert_eq!(back.reasoning, None);
    assert!(back.budget.is_none());
}

#[test]
fn a_pending_override_rejects_an_unknown_field() {
    let error = serde_json::from_str::<PendingOverride>(r#"{"sampling":{"temperature":0.2}}"#)
        .expect_err("an unknown override field must be rejected");
    assert!(
        error.to_string().contains("unknown field"),
        "unexpected error: {error}"
    );
}
