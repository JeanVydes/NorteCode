use norte_code_wire::{BudgetAction, BudgetUnit, ServerEventKind};

use super::App;
use crate::ui_model::TranscriptItem;

#[test]
fn compaction_status_omits_the_summary() {
    let status = App::transcript_status(&ServerEventKind::CompactionFinished {
        compacted: true,
        north_session: Some("new-engine-session".to_owned()),
        summary: Some("private compaction summary".to_owned()),
        error: None,
    });

    assert!(matches!(
        status,
        Some(TranscriptItem::Status(ref message)) if message == "Session compacted."
    ));
}

#[test]
fn a_warning_budget_is_reported_as_non_blocking() {
    let status = App::transcript_status(&ServerEventKind::BudgetChanged {
        budget: Some(norte_code_wire::Budget {
            unit: BudgetUnit::Usd,
            limit: 3.5,
            action: BudgetAction::Warn,
        }),
    });

    assert!(matches!(
        status,
        Some(TranscriptItem::Status(ref message))
            if message.contains("$3.50")
                && message.contains("warning only, continues")
    ));
}

#[test]
fn an_ask_budget_is_reported_as_an_approval_gate() {
    let status = App::transcript_status(&ServerEventKind::BudgetExceeded {
        scope: norte_code_wire::BudgetScope::Session,
        unit: BudgetUnit::Usd,
        spent: 4.0,
        limit: 3.5,
        action: BudgetAction::Ask,
    });

    assert!(matches!(
        status,
        Some(TranscriptItem::Status(ref message))
            if message.contains("requires approval")
    ));
}

#[test]
fn a_model_change_names_the_model_the_session_runs_on() {
    let change = App::model_change(&ServerEventKind::ModelChanged {
        provider: "openai".to_owned(),
        model: "gpt-5".to_owned(),
    });

    let (model, message) = change.expect("model change");
    assert_eq!(model.provider, "openai");
    assert_eq!(model.model, "gpt-5");
    assert!(message.contains("openai/gpt-5"));
}

#[test]
fn a_model_change_that_waits_for_the_turn_is_announced() {
    let (_, message) = App::model_change_pending(&ServerEventKind::ModelQueued {
        provider: "openai".to_owned(),
        model: "gpt-5".to_owned(),
    })
    .expect("queued model change");

    assert!(message.contains("after this turn"));
}

#[test]
fn an_error_kills_a_live_turn_but_not_an_idle_one() {
    let error = ServerEventKind::Error {
        code: norte_code_wire::ErrorCode::Provider,
        message: "rate limit".to_owned(),
    };

    assert!(App::is_turn_ending_error(
        &error,
        crate::ui_model::TurnState::Thinking
    ));
    assert!(App::is_turn_ending_error(
        &error,
        crate::ui_model::TurnState::Streaming
    ));
    assert!(!App::is_turn_ending_error(
        &error,
        crate::ui_model::TurnState::Idle
    ));
}

#[test]
fn a_sequence_gap_is_not_a_turn_error() {
    assert!(App::is_sequence_gap(
        "RAEP sequence gap: expected 1334, received 1335"
    ));
    assert!(!App::is_sequence_gap("rate limit"));
}

#[test]
fn the_task_list_the_host_sends_is_kept() {
    let todos = App::todo_list(&ServerEventKind::SessionTodos {
        todos: vec![norte_code_wire::TodoInfo {
            content: "read the file".to_owned(),
            status: norte_code_wire::TodoStatus::InProgress,
        }],
    });

    let todos = todos.expect("task list");
    assert_eq!(todos.len(), 1);
    assert_eq!(todos[0].content, "read the file");
}

#[test]
fn a_reconstructed_request_accumulates_into_the_session_stats() {
    let mut stats = crate::ui_model::SessionStatsSummary {
        requests: 1,
        input_tokens: 1300,
        output_tokens: 400,
        cache_read_tokens: 100,
        cache_write_tokens: 50,
        cost_usd: 0.5,
        context_used: 1300,
        context_window: Some(128_000),
    };
    App::session_stats_update(
        &mut stats,
        &ServerEventKind::RequestReconstructed {
            turn: 1,
            index: 1,
            usage: norte_code_wire::Usage {
                input_tokens: Some(1200),
                output_tokens: Some(340),
                cache_read_tokens: Some(200),
                cache_write_tokens: Some(20),
                cost_usd: Some(0.25),
                ..norte_code_wire::Usage::default()
            },
            cost_usd: Some(0.25),
            truncated: false,
            duration_ms: 820,
            first_token_ms: Some(120),
            tokens_per_second: Some(42.5),
            cache_hit: true,
            provider: Some("openai".to_owned()),
            model: Some("gpt-5".to_owned()),
            tools: Vec::new(),
        },
    );

    assert_eq!(stats.requests, 2);
    assert_eq!(stats.input_tokens, 2500);
    assert_eq!(stats.output_tokens, 740);
    assert_eq!(stats.cache_read_tokens, 300);
    assert_eq!(stats.cache_write_tokens, 70);
    assert!((stats.cost_usd - 0.75).abs() < 1e-9, "{}", stats.cost_usd);
    assert_eq!(stats.context_used, 1200);
    assert_eq!(stats.context_window, Some(128_000));
}

#[test]
fn a_resume_snapshot_replaces_the_accumulated_stats() {
    let mut stats = crate::ui_model::SessionStatsSummary {
        requests: 1,
        input_tokens: 1000,
        ..crate::ui_model::SessionStatsSummary::default()
    };
    App::session_stats_update(
        &mut stats,
        &ServerEventKind::SessionStats {
            input_tokens: 9000,
            output_tokens: 700,
            cache_read_tokens: 800,
            cache_write_tokens: 100,
            cost_usd: 1.5,
            requests: 9,
            context_used: 9000,
            context_window: Some(200_000),
        },
    );

    assert_eq!(stats.requests, 9);
    assert_eq!(stats.input_tokens, 9000);
    assert_eq!(stats.cost_usd, 1.5);
    assert_eq!(stats.context_window, Some(200_000));
}

#[test]
fn the_transport_notice_names_the_host_the_request_was_for() {
    let message = "RAEP event stream failed: error sending request for url (http://127.0.0.1:8787/raep/v1/sessions/15ff4519-fd9a-4050-8b50-522bb5f3f394/events?agent_id=norte-code)";

    assert_eq!(
        App::transport_notice(message),
        "Service unreachable (127.0.0.1:8787); retrying in the background."
    );
}

#[test]
fn the_transport_notice_stands_without_a_host_in_the_message() {
    assert_eq!(
        App::transport_notice("error sending request for url"),
        "Service unreachable; retrying in the background."
    );
}

#[test]
fn a_transport_failure_is_a_request_that_never_left() {
    let down = [
        "RAEP event stream failed: error sending request for url (http://127.0.0.1:8787/events)",
        "Request failed: error sending request for url (http://127.0.0.1:8787/children)",
        "RAEP attach failed: error sending request for url (http://127.0.0.1:8787/commands)",
        "RAEP command failed: error sending request for url (http://127.0.0.1:8787/commands)",
    ];
    for message in down {
        assert!(App::is_transport_failure(message), "{message}");
    }
    assert!(!App::is_transport_failure(
        "The host refused the request: no such session"
    ));
    assert!(!App::is_transport_failure("Server returned HTTP 503"));
}
