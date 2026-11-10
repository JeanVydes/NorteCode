use norte_code_wire::*;

#[test]
fn a_request_round_trips() {
    let request = ClientRequest {
        request_id: 3,
        kind: ClientRequestKind::SendMessage {
            session: Some(SessionKey::new("abc")),
            text: "hi".to_owned(),
            attachments: Vec::new(),
            budget: Some(Budget {
                unit: BudgetUnit::Usd,
                limit: 1.0,
                action: BudgetAction::Stop,
            }),
            reasoning: Some(Reasoning::Effort {
                effort: "high".to_owned(),
            }),
            output: Vec::new(),
            response_format: None,
            metadata: None,
            replay: false,
        },
    };
    let json = serde_json::to_string(&request).expect("serialize");
    let back: ClientRequest = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.request_id, 3);
}

#[test]
fn an_event_round_trips() {
    let event = ServerEvent::new(
        1,
        Some(SessionKey::new("s")),
        ServerEventKind::Delta {
            turn: 1,
            delta: Delta::Text {
                index: 0,
                text: "hi".to_owned(),
            },
        },
    );
    let json = serde_json::to_string(&event).expect("serialize");
    let back: ServerEvent = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.session.expect("session").as_str(), "s");
}

#[test]
fn a_steer_applied_event_round_trips() {
    let event = ServerEvent::new(
        1,
        Some(SessionKey::new("s")),
        ServerEventKind::SteerApplied {
            ids: vec!["one".to_owned()],
            messages: vec!["hello".to_owned()],
        },
    );
    let json = serde_json::to_string(&event).expect("serialize");
    let back: ServerEvent = serde_json::from_str(&json).expect("deserialize");
    assert!(matches!(
        back.kind,
        ServerEventKind::SteerApplied { ids, messages }
            if ids == ["one"] && messages == ["hello"]
    ));
}

#[test]
fn a_retry_event_round_trips() {
    let event = ServerEvent::new(
        1,
        Some(SessionKey::new("s")),
        ServerEventKind::RetryScheduled {
            turn: 3,
            attempt: 2,
            delay_ms: 1_500,
            reason: "rate limited".to_owned(),
        },
    );
    let json = serde_json::to_string(&event).expect("serialize");
    assert!(json.contains("\"event\":\"retry_scheduled\""));
    let back: ServerEvent = serde_json::from_str(&json).expect("deserialize");
    match back.kind {
        ServerEventKind::RetryScheduled {
            turn,
            attempt,
            delay_ms,
            reason,
        } => {
            assert_eq!((turn, attempt, delay_ms), (3, 2, 1_500));
            assert_eq!(reason, "rate limited");
        }
        other => panic!("expected a retry, got {other:?}"),
    }
}

#[test]
fn a_retry_start_round_trips() {
    let event = ServerEvent::new(
        1,
        Some(SessionKey::new("s")),
        ServerEventKind::RetryStarted {
            turn: 3,
            attempt: 2,
        },
    );
    let json = serde_json::to_string(&event).expect("serialize");
    assert!(json.contains("\"event\":\"retry_started\""));
    let back: ServerEvent = serde_json::from_str(&json).expect("deserialize");
    assert!(matches!(
        back.kind,
        ServerEventKind::RetryStarted {
            turn: 3,
            attempt: 2
        }
    ));
}
