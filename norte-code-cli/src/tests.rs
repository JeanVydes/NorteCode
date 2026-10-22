use super::{
    ApprovalChoice, Cli, Command, DenyScope, GrantScope, PreviewLine, SandboxOptions, Style,
    apply_extensions, approval_request, argument_preview, budget_label, compose_selection,
    cost_summary, parse_approval_choice, parse_approval_command, parse_reasoning_setting,
    parse_selection, select_system_prompt,
};
use crate::commands::{RequestSummary, bucket_label, telemetry_table};
use chrono::{DateTime, Utc};
use clap::Parser;
use norte_code_config::Config;
use norte_code_cost::CostSummary;
use norte_code_protocol::{Budget, BudgetAction, BudgetUnit, ClientRequestKind, SessionKey};
use norte_code_store_sqlite::RequestRecord;

/// A request row for the telemetry tests.
fn telemetry_record(
    provider: &str,
    model: &str,
    first_token_ms: Option<u64>,
    reasoning_tokens: Option<u64>,
) -> RequestRecord {
    RequestRecord {
        id: "id".to_owned(),
        session: "session".to_owned(),
        north_session: "north".to_owned(),
        turn: 1,
        provider: provider.to_owned(),
        model: model.to_owned(),
        started_at: DateTime::<Utc>::from_timestamp(0, 0).expect("epoch"),
        duration_ms: 1_000,
        first_token_ms,
        last_token_ms: None,
        input_tokens: Some(100),
        output_tokens: Some(10),
        reasoning_tokens,
        cache_read_tokens: None,
        cache_write_tokens: None,
        estimated_input_tokens: None,
        cache_hit: false,
        tool_calls: Vec::new(),
        reasoning: None,
        params: serde_json::Value::Null,
        extensions: serde_json::Value::Null,
        metadata: serde_json::Value::Null,
        cost_usd: Some(0.01),
        finish_reason: None,
        status: "ok".to_owned(),
        error: None,
        sandbox: false,
        sandbox_network: false,
    }
}

#[test]
fn the_telemetry_table_shows_the_first_token_the_reasoning_and_the_provider() {
    let mut summary = RequestSummary::default();
    summary.add(&telemetry_record("openai", "gpt-5", Some(300), Some(120)));
    summary.add(&telemetry_record("anthropic", "claude", Some(900), None));

    let table = telemetry_table(Style::plain(), "by day", &[("2026-10-21", &summary)], true);
    assert!(table.contains("ttft 600ms"), "{table}");
    assert!(table.contains("120"), "{table}");
    assert!(table.contains("anthropic+openai"), "{table}");
    assert!(table.contains("2 req"), "{table}");
    // The header names the columns, so a row of numbers says what it measures.
    assert!(table.contains("requests"), "{table}");
    assert!(table.contains("cache"), "{table}");

    let json = summary.json();
    assert_eq!(json["average_ttft_ms"], 600);
    assert_eq!(json["p95_ttft_ms"], 900);
    assert_eq!(json["reasoning_tokens"], 120);
    assert_eq!(
        json["providers"],
        serde_json::json!(["anthropic", "openai"])
    );
}

#[test]
fn the_telemetry_cache_rate_is_weighted_by_tokens() {
    // A request that hit the cache for a thousand tokens and one that missed:
    // counting requests would say half, counting tokens says a quarter.
    let mut summary = RequestSummary::default();
    let mut hit = telemetry_record("openai", "gpt-5", Some(300), None);
    hit.input_tokens = Some(4_000);
    hit.cache_read_tokens = Some(1_000);
    summary.add(&hit);
    let mut miss = telemetry_record("openai", "gpt-5", Some(300), None);
    miss.input_tokens = Some(4_000);
    miss.cache_read_tokens = Some(0);
    summary.add(&miss);
    assert_eq!(summary.cache_hit_percent(), Some(12));

    let table = telemetry_table(
        Style::plain(),
        "by session",
        &[("session-a", &summary)],
        true,
    );
    assert!(table.contains("12%"), "{table}");
}

#[test]
fn the_telemetry_table_of_one_provider_does_not_name_it_twice() {
    let mut summary = RequestSummary::default();
    summary.add(&telemetry_record("openai", "gpt-5", Some(300), None));

    let table = telemetry_table(
        Style::plain(),
        "by provider",
        &[("openai", &summary)],
        false,
    );
    assert!(table.contains("ttft 300ms"), "{table}");
    assert_eq!(table.matches("openai").count(), 1, "{table}");
}

#[test]
fn the_telemetry_buckets_name_the_period_they_group() {
    let at = DateTime::<Utc>::from_timestamp(1_800_000_000, 0).expect("epoch");
    assert_eq!(bucket_label(at, "hour"), "2027-01-15 08:00");
    assert_eq!(bucket_label(at, "day"), "2027-01-15");
    assert_eq!(bucket_label(at, "week"), "2027-W02");
    assert_eq!(bucket_label(at, "month"), "2027-01");
    assert_eq!(bucket_label(at, "year"), "2027");
    assert_eq!(bucket_label(at, "total"), "total");
}

#[test]
fn sandbox_flags_override_the_config_and_respect_the_network() {
    let mut config = Config::default();
    assert!(!config.tools.sandbox.enabled);
    let enable = SandboxOptions {
        sandbox: true,
        no_sandbox: false,
        sandbox_network: None,
    };
    enable.apply(&mut config).expect("enable");
    assert!(config.tools.sandbox.enabled);

    let networked = SandboxOptions {
        sandbox: false,
        no_sandbox: false,
        sandbox_network: Some("on".to_owned()),
    };
    networked.apply(&mut config).expect("network on");
    assert!(config.tools.sandbox.network);

    let offline = SandboxOptions {
        sandbox: false,
        no_sandbox: false,
        sandbox_network: Some("off".to_owned()),
    };
    offline.apply(&mut config).expect("network off");
    assert!(!config.tools.sandbox.network);

    let disable = SandboxOptions {
        sandbox: false,
        no_sandbox: true,
        sandbox_network: None,
    };
    disable.apply(&mut config).expect("disable");
    assert!(!config.tools.sandbox.enabled);
}

#[test]
fn sandbox_flags_reject_contradictions_and_a_network_without_a_sandbox() {
    let mut config = Config::default();
    let both = SandboxOptions {
        sandbox: true,
        no_sandbox: true,
        sandbox_network: None,
    };
    assert!(both.apply(&mut config).is_err());

    let denied = SandboxOptions {
        sandbox: false,
        no_sandbox: true,
        sandbox_network: Some("on".to_owned()),
    };
    assert!(denied.apply(&mut config).is_err());

    let orphan = SandboxOptions {
        sandbox: false,
        no_sandbox: false,
        sandbox_network: Some("on".to_owned()),
    };
    assert!(orphan.apply(&mut config).is_err());
}

#[test]
fn the_system_prompt_prefers_the_flag_then_the_config() {
    assert_eq!(
        select_system_prompt(Some("flag".to_owned()), Some("config".to_owned())),
        ("flag", "flag".to_owned())
    );
    assert_eq!(
        select_system_prompt(None, Some("config".to_owned())),
        ("config", "config".to_owned())
    );
    let (source, prompt) = select_system_prompt(None, None);
    assert_eq!(source, "default");
    assert!(prompt.contains("Norte Code"));
}

#[test]
fn the_default_prompt_asks_for_the_todo_tool() {
    let (_, prompt) = select_system_prompt(None, None);
    assert!(prompt.contains("`todo`"));
    assert!(prompt.contains("in_progress"));
    assert!(prompt.contains("footer"));
}

#[test]
fn reasoning_names_are_parsed_and_unknown_ones_are_rejected() {
    assert_eq!(
        parse_reasoning_setting("off").expect("off"),
        norte_code_protocol::Reasoning::Disabled
    );
    assert_eq!(
        parse_reasoning_setting("xhigh").expect("xhigh"),
        norte_code_protocol::Reasoning::Effort {
            effort: "xhigh".to_owned()
        }
    );
    assert_eq!(
        parse_reasoning_setting("max").expect("max"),
        norte_code_protocol::Reasoning::Effort {
            effort: "max".to_owned()
        }
    );
    assert!(parse_reasoning_setting("x").is_err());
}

#[test]
fn vendor_extensions_are_parsed_into_the_config() {
    let mut config = norte_code_config::Config::default();
    apply_extensions(
        &mut config,
        &[
            r#"openai.chat:provider.order=["deepinfra"]"#.to_owned(),
            "openai.chat:temperature=0.2".to_owned(),
            "openai.chat:metadata=review".to_owned(),
        ],
    )
    .expect("extensions");
    assert_eq!(
        config.extensions["openai.chat:provider.order"],
        serde_json::json!(["deepinfra"])
    );
    assert_eq!(
        config.extensions["openai.chat:temperature"],
        serde_json::json!(0.2)
    );
    assert_eq!(
        config.extensions["openai.chat:metadata"],
        serde_json::json!("review")
    );
    assert!(apply_extensions(&mut config, &["provider".to_owned()]).is_err());
    assert!(apply_extensions(&mut config, &["openai.chat:provider".to_owned()]).is_err());
}

#[test]
fn closed_question_selections_are_parsed() {
    let options = ["Read".to_owned(), "Write".to_owned()];
    assert_eq!(
        parse_selection("1", &options, false),
        Some((vec![0], false))
    );
    assert_eq!(
        parse_selection("write", &options, false),
        Some((vec![1], false))
    );
    assert_eq!(
        parse_selection("3", &options, false),
        Some((Vec::new(), true))
    );
    assert_eq!(
        parse_selection("1, 2", &options, true),
        Some((vec![0, 1], false))
    );
    assert_eq!(
        parse_selection("1, 3", &options, true),
        Some((vec![0], true))
    );
    assert_eq!(
        parse_selection("all", &options, true),
        Some((vec![0, 1], false))
    );
    assert_eq!(parse_selection("1, 2", &options, false), None);
    assert_eq!(parse_selection("0", &options, false), None);
    assert_eq!(
        compose_selection(&options, &[0, 1], Some("something custom")),
        "Read, Write, something custom"
    );
}

#[test]
fn chat_approval_commands_default_to_the_session_and_reject_bad_scopes() {
    assert_eq!(
        parse_approval_command("/allow command"),
        Some(Ok((true, "command".to_owned(), false)))
    );
    assert_eq!(
        parse_approval_command("/deny write global"),
        Some(Ok((false, "write".to_owned(), true)))
    );
    assert_eq!(
        parse_approval_command("/allow skill_script session"),
        Some(Ok((true, "skill_script".to_owned(), false)))
    );
    assert!(parse_approval_command("/allow").expect("command").is_err());
    assert!(
        parse_approval_command("/deny write everywhere")
            .expect("command")
            .is_err()
    );
    assert!(
        parse_approval_command("/allow write global extra")
            .expect("command")
            .is_err()
    );
    assert_eq!(parse_approval_command("/allowance"), None);
    assert_eq!(parse_approval_command("/other command"), None);
    assert_eq!(parse_approval_command("hello"), None);
}

#[test]
fn approval_answers_are_parsed_leniently() {
    assert_eq!(parse_approval_choice("y"), Some(ApprovalChoice::Once));
    assert_eq!(parse_approval_choice(" YES "), Some(ApprovalChoice::Once));
    assert_eq!(
        parse_approval_choice("s"),
        Some(ApprovalChoice::ToolSession)
    );
    assert_eq!(parse_approval_choice("A"), Some(ApprovalChoice::ToolGlobal));
    assert_eq!(
        parse_approval_choice("always"),
        Some(ApprovalChoice::ToolGlobal)
    );
    assert_eq!(
        parse_approval_choice("n"),
        Some(ApprovalChoice::DenyOnce(None))
    );
    assert_eq!(
        parse_approval_choice(""),
        Some(ApprovalChoice::DenyOnce(None))
    );
    assert_eq!(
        parse_approval_choice("ns"),
        Some(ApprovalChoice::DenySession(None))
    );
    assert_eq!(
        parse_approval_choice("ng"),
        Some(ApprovalChoice::DenyGlobal(None))
    );
    assert_eq!(parse_approval_choice("maybe"), None);
}

#[test]
fn a_denial_can_carry_feedback_for_the_model() {
    assert_eq!(
        parse_approval_choice("n use the existing helper"),
        Some(ApprovalChoice::DenyOnce(Some(
            "use the existing helper".to_owned()
        )))
    );
    assert_eq!(
        parse_approval_choice("no this touches production"),
        Some(ApprovalChoice::DenyOnce(Some(
            "this touches production".to_owned()
        )))
    );
    assert_eq!(
        parse_approval_choice("ns use the helper"),
        Some(ApprovalChoice::DenySession(Some(
            "use the helper".to_owned()
        )))
    );
    assert_eq!(
        parse_approval_choice("ng not in this repo"),
        Some(ApprovalChoice::DenyGlobal(Some(
            "not in this repo".to_owned()
        )))
    );
    assert_eq!(parse_approval_choice("f"), None);
}

#[test]
fn a_read_preview_names_the_path_instead_of_dumping_json() {
    let arguments = serde_json::json!({"path": "src/main.rs"});
    assert_eq!(
        argument_preview("read", &arguments),
        vec![PreviewLine::Field {
            key: "path".to_owned(),
            value: "src/main.rs".to_owned(),
        }]
    );
}

#[test]
fn a_command_preview_shows_the_script_and_its_shell() {
    let arguments = serde_json::json!({
        "command": {"kind": "shell", "shell": "sh", "script": "cargo test\ncargo clippy"},
        "cwd": "/tmp/project",
    });
    let lines = argument_preview("command", &arguments);
    assert!(lines.contains(&PreviewLine::Field {
        key: "shell".to_owned(),
        value: "sh".to_owned(),
    }));
    assert!(lines.contains(&PreviewLine::Field {
        key: "cwd".to_owned(),
        value: "/tmp/project".to_owned(),
    }));
    assert!(lines.contains(&PreviewLine::Block {
        label: "script".to_owned(),
        lines: vec!["cargo test".to_owned(), "cargo clippy".to_owned()],
    }));
}

#[test]
fn write_content_is_bounded() {
    let content = (0..20)
        .map(|index| format!("line {index}"))
        .collect::<Vec<_>>()
        .join("\n");
    let arguments = serde_json::json!({"path": "src/main.rs", "content": content});
    let lines = argument_preview("write", &arguments);
    let block = lines
        .iter()
        .find_map(|line| match line {
            PreviewLine::Block { label, lines } if label == "content" => Some(lines),
            _ => None,
        })
        .expect("content block");
    assert_eq!(block.len(), 13);
    assert!(block.last().is_some_and(|line| line.contains("more line")));
}

#[test]
fn previews_escape_terminal_controls() {
    let arguments = serde_json::json!({
        "path": "safe\u{1b}[2Jowned",
        "content": "first\nsecond\u{202e}spoofed",
    });
    let lines = argument_preview("write", &arguments);
    assert!(lines.iter().any(|line| matches!(
        line,
        PreviewLine::Field { key, .. } if key == "path"
    )));
    assert!(lines.iter().any(|line| matches!(
        line,
        PreviewLine::Block { label, .. } if label == "content"
    )));
    for line in lines {
        let text = match line {
            PreviewLine::Field { key, value } => format!("{key}{value}"),
            PreviewLine::Block { label, lines } => {
                format!("{label}{}", lines.join(""))
            }
            PreviewLine::Note(note) => note,
        };
        assert!(!text.contains('\u{1b}'), "ESC reached the preview: {text}");
        assert!(
            !text.contains('\u{202e}'),
            "bidi override reached the preview: {text}"
        );
    }
}

#[test]
fn cost_totals_show_cache_and_the_input_output_ratio() {
    let mixed = CostSummary {
        requests: 2,
        input_tokens: 1_000,
        output_tokens: 100,
        cache_read_tokens: 900,
        reported_cost_usd: 0.5,
        not_reported: 1,
        ..CostSummary::default()
    };
    assert_eq!(
        cost_summary(&mixed),
        "2 request(s) · 1k in · 100 out · cache 90% · in:out 10:1 · $0.500000 · 1 not reported"
    );
    let unpriced = CostSummary {
        requests: 2,
        not_reported: 2,
        ..CostSummary::default()
    };
    assert_eq!(
        cost_summary(&unpriced),
        "2 request(s) · 0 in · 0 out · in:out – · cost not reported"
    );
}

#[test]
fn budget_labels_say_what_is_measured_and_what_happens() {
    assert_eq!(
        budget_label(&Budget {
            unit: BudgetUnit::Requests,
            limit: 12.0,
            action: BudgetAction::Stop,
        }),
        "12 requests (stop)"
    );
}

#[test]
fn an_interactive_run_can_start_without_a_prompt() {
    let cli = Cli::try_parse_from(["norte-code", "run", "--interactive"])
        .expect("interactive run should parse");
    match cli.command {
        Command::Run(args) => {
            assert!(args.interactive);
            assert!(args.prompt.is_none());
        }
        _ => panic!("expected run"),
    }
}

#[test]
fn a_headless_run_still_requires_a_prompt() {
    assert!(Cli::try_parse_from(["norte-code", "run"]).is_err());
}

#[test]
fn approval_scopes_are_explicit_in_the_request() {
    let session = SessionKey::new("s");
    let session_allow = approval_request(
        session.clone(),
        "approval".to_owned(),
        ApprovalChoice::ToolSession,
    );
    match session_allow.kind {
        ClientRequestKind::Approve {
            scope, remember, ..
        } => {
            assert!(matches!(scope, Some(GrantScope::Tool)));
            assert_eq!(remember, Some(true));
        }
        other => panic!("expected an approval, got {other:?}"),
    }
    let global_allow = approval_request(
        session.clone(),
        "approval".to_owned(),
        ApprovalChoice::ToolGlobal,
    );
    match global_allow.kind {
        ClientRequestKind::Approve { scope, .. } => {
            assert!(matches!(scope, Some(GrantScope::Always)));
        }
        other => panic!("expected an approval, got {other:?}"),
    }
    let deny = approval_request(
        session,
        "approval".to_owned(),
        ApprovalChoice::DenySession(Some("use the helper".to_owned())),
    );
    match deny.kind {
        ClientRequestKind::Deny { reason, scope, .. } => {
            assert_eq!(reason.as_deref(), Some("use the helper"));
            assert_eq!(scope, Some(DenyScope::ToolSession));
        }
        other => panic!("expected a denial, got {other:?}"),
    }
}
