use super::{
    ApprovalChoice, Cli, Command, DenyScope, GrantScope, PreviewLine, SandboxOptions,
    apply_extensions, approval_request, argument_preview, budget_label, compose_selection,
    cost_summary, parse_approval_choice, parse_approval_command, parse_reasoning_setting,
    parse_selection, select_system_prompt,
};
use clap::Parser;
use norte_code_config::Config;
use norte_code_cost::CostSummary;
use norte_code_protocol::{Budget, BudgetAction, BudgetUnit, ClientRequestKind, SessionKey};

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
fn reasoning_names_are_parsed_and_unknown_ones_are_rejected() {
    assert_eq!(
        parse_reasoning_setting("off").expect("off"),
        norte_code_protocol::Reasoning::Disabled
    );
    assert_eq!(
        parse_reasoning_setting("xhigh").expect("alias"),
        norte_code_protocol::Reasoning::Effort {
            effort: "high".to_owned()
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
