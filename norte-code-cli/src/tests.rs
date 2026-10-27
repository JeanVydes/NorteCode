use super::{
    ApprovalChoice, Cli, Command, DenyScope, GrantScope, PreviewLine, SandboxOptions, Style,
    apply_extensions, approval_request, argument_preview, budget_label, budget_warning,
    compose_selection, cost_summary, parse_approval_choice, parse_reasoning_setting,
    select_system_prompt,
};
use crate::commands::{RequestSummary, bucket_label, telemetry_table};
use chrono::{DateTime, Utc};
use clap::Parser;
use norte_code_config::{AuthKind, BudgetConfig, Config, WireKind};
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
        endpoint: None,
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
        error_kind: None,
        retry_after_ms: None,
        sandbox: false,
        sandbox_network: false,
    }
}

#[test]
fn a_usd_budget_on_a_cost_silent_provider_is_flagged() {
    let config = Config::default();
    assert!(budget_warning("google", &config, true).is_some());
    assert!(budget_warning("anthropic", &config, true).is_some());
    assert!(budget_warning("openrouter", &config, true).is_none());
    assert!(budget_warning("google", &config, false).is_none());

    let mut config = Config::default();
    config.budgets.session = Some(BudgetConfig {
        unit: "usd".to_owned(),
        limit: 1.0,
        action: "stop".to_owned(),
    });
    assert!(budget_warning("google", &config, false).is_some());
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
    assert!(!config.tools.confinement.sandbox.enabled);
    let enable = SandboxOptions {
        sandbox: true,
        no_sandbox: false,
        sandbox_network: None,
    };
    enable.apply(&mut config).expect("enable");
    assert!(config.tools.confinement.sandbox.enabled);

    let networked = SandboxOptions {
        sandbox: false,
        no_sandbox: false,
        sandbox_network: Some("on".to_owned()),
    };
    networked.apply(&mut config).expect("network on");
    assert!(config.tools.confinement.sandbox.network);

    let offline = SandboxOptions {
        sandbox: false,
        no_sandbox: false,
        sandbox_network: Some("off".to_owned()),
    };
    offline.apply(&mut config).expect("network off");
    assert!(!config.tools.confinement.sandbox.network);

    let disable = SandboxOptions {
        sandbox: false,
        no_sandbox: true,
        sandbox_network: None,
    };
    disable.apply(&mut config).expect("disable");
    assert!(!config.tools.confinement.sandbox.enabled);
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
fn a_selection_is_composed_as_the_one_answer_the_model_reads() {
    // The choices are taken by the cursor now, so what is left to check is how
    // they reach the model: one answer, in the order they were taken.
    let options = ["Read".to_owned(), "Write".to_owned()];
    assert_eq!(compose_selection(&options, &[0], None), "Read");
    assert_eq!(compose_selection(&options, &[1, 0], None), "Write, Read");
    assert_eq!(
        compose_selection(&options, &[0, 1], Some("something custom")),
        "Read, Write, something custom"
    );
    // Nothing taken and nothing typed is an empty answer, which the caller
    // refuses rather than sending: it is not a choice someone made.
    assert_eq!(compose_selection(&options, &[], None), "");
}

#[test]
fn chat_approval_commands_default_to_the_session_and_reject_bad_scopes() {
    // What a rule names is a tool, and the completion offers none of them here:
    // which tools exist is the engine's answer and not a list in the client.
    let registry = crate::commands_registry::shared();
    let allow = registry
        .parse("/allow command")
        .expect("a command")
        .expect("a tool and no scope");
    assert_eq!(allow.command, "/allow");
    assert_eq!(allow.arg("tool"), Some("command"));
    assert_eq!(
        allow.arg("scope"),
        None,
        "the scope defaults to the session"
    );

    let deny = registry
        .parse("/deny write global")
        .expect("a command")
        .expect("a tool and a scope");
    assert_eq!(deny.command, "/deny");
    assert_eq!(deny.arg("tool"), Some("write"));
    assert_eq!(deny.arg("scope"), Some("global"));

    let alias_like = registry
        .parse("/allow skill_script session")
        .expect("a command")
        .expect("an underscore is part of a name");
    assert_eq!(alias_like.arg("tool"), Some("skill_script"));

    assert!(
        registry.parse("/allow").expect("a command").is_err(),
        "the tool is required"
    );
    assert!(
        registry
            .parse("/deny write everywhere")
            .expect("a command")
            .is_err(),
        "an unknown scope is refused rather than sent to the model"
    );
    assert!(
        registry
            .parse("/allow write global extra")
            .expect("a command")
            .is_err(),
        "a word nobody takes is not dropped"
    );
    assert!(
        registry.parse("/allowance").is_none(),
        "a word that only starts like one is not a command"
    );
    assert!(registry.parse("/other command").is_none());
    assert!(registry.parse("hello").is_none());
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
        "script": "cargo test\ncargo clippy",
        "shell": "sh",
        "cwd": "/tmp/project",
    });
    let lines = argument_preview("shell", &arguments);
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
        "2 request(s) · 1k in · 100 out · cache 90.00% · in:out 10:1 · $0.500000 · 1 not reported"
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
fn wire_and_auth_flags_are_parsed_and_validated() {
    let cli = Cli::try_parse_from([
        "norte-code",
        "new",
        "--wire",
        "messages",
        "--auth",
        "bearer",
    ])
    .expect("parse");
    let Some(Command::New(args)) = cli.command else {
        panic!("expected new");
    };
    assert_eq!(args.wire, Some(WireKind::Messages));
    assert_eq!(args.auth, Some(AuthKind::Bearer));

    let cli = Cli::try_parse_from(["norte-code", "new", "--auth", "disabled"])
        .expect("parse disabled auth");
    let Some(Command::New(args)) = cli.command else {
        panic!("expected new");
    };
    assert_eq!(args.auth, Some(AuthKind::Disabled));

    assert!(Cli::try_parse_from(["norte-code", "new", "--wire", "telepathy"]).is_err());
    assert!(Cli::try_parse_from(["norte-code", "new", "--auth", "none"]).is_err());
}

#[test]
fn a_new_session_takes_no_prompt() {
    // The first line typed is the first message, so there is nothing to pass on
    // the command line: a stray word is a usage error rather than a prompt that
    // would be sent before the session is read.
    assert!(Cli::try_parse_from(["norte-code", "new", "hi there"]).is_err());
    // The session is what the command starts, so it is not named either.
    assert!(Cli::try_parse_from(["norte-code", "new", "--session", "abc"]).is_err());
    let cli = Cli::try_parse_from(["norte-code", "new"]).expect("a bare new parses");
    let Some(Command::New(args)) = cli.command else {
        panic!("expected new");
    };
    // The model is optional: without it, `default_model` decides.
    assert!(args.model.is_none() && args.provider.is_none());
}

#[test]
fn a_new_session_can_override_the_model_and_the_provider() {
    let cli = Cli::try_parse_from(["norte-code", "new", "--model", "openai/gpt-5"])
        .expect("model override");
    let Some(Command::New(args)) = cli.command else {
        panic!("expected new");
    };
    assert_eq!(args.model.as_deref(), Some("openai/gpt-5"));

    let cli = Cli::try_parse_from(["norte-code", "new", "--provider", "river", "-m", "deepseek"])
        .expect("provider and model");
    let Some(Command::New(args)) = cli.command else {
        panic!("expected new");
    };
    assert_eq!(args.provider.as_deref(), Some("river"));
    assert_eq!(args.model.as_deref(), Some("deepseek"));
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

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn approving_a_write_resumes_the_turn_instead_of_parking_it() {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use futures_util::StreamExt;
    use norte::harness::{InMemoryBlobStore, InMemoryStore};
    use norte::{CallId, Capabilities, Model, ModelId, ModelRef, ProviderId};
    use norte_code_agent::{EventStream, ModelFactory, ModelSpec, Service, ServiceError, Wire};
    use norte_code_catalog::{Catalog, capabilities};
    use norte_code_config::ApiKey;
    use norte_code_protocol::{ClientRequest, ServerEventKind};
    use norte_testing::ScriptedModel;
    use tokio::sync::mpsc;

    use crate::input::Pinned;
    use crate::interaction::drive_turns;
    use crate::render::{Format, Renderer, Verbosity};
    use crate::style::Style;

    struct ScriptedFactory {
        model: Arc<dyn Model>,
    }

    impl ModelFactory for ScriptedFactory {
        fn build(
            &self,
            _spec: &ModelSpec,
            _capabilities: &Capabilities,
        ) -> Result<Arc<dyn Model>, ServiceError> {
            Ok(Arc::clone(&self.model))
        }
    }

    let target = std::env::current_dir()
        .expect("working directory")
        .join("target")
        .join(format!(
            "norte-code-cli-approval-{}.txt",
            std::process::id()
        ));
    let _ = std::fs::remove_file(&target);

    let catalog = Arc::new(
        Catalog::parse(
            br#"{
                "providers": {
                    "test": {
                        "id": "test",
                        "env": ["TEST_API_KEY"],
                        "npm": "@ai-sdk/openai-compatible",
                        "api": "https://example.invalid/v1",
                        "models": {
                            "scripted": {
                                "id": "scripted",
                                "name": "Scripted",
                                "tool_call": true,
                                "modalities": {"input": ["text"], "output": ["text"]},
                                "limit": {"context": 1000, "output": 100}
                            }
                        }
                    }
                }
            }"#,
        )
        .expect("catalog"),
    );
    // A unique name, written atomically: a shared file could be read while
    // another test truncated it, and the reader would see an empty key.
    let key_file = std::env::temp_dir().join(format!(
        "norte-code-cli-approval-{}-key",
        std::process::id()
    ));
    let key_tmp = key_file.with_extension("tmp");
    std::fs::write(&key_tmp, "sk-test\n").expect("key file");
    std::fs::rename(&key_tmp, &key_file).expect("key file");
    let config = Config::from_json(&format!(
        r#"{{"providers":{{"test":{{"api_key_file":"{}"}}}}}}"#,
        key_file.display()
    ))
    .expect("config")
    .config;
    let entry = catalog.find("test", "scripted").expect("model");
    let spec = ModelSpec {
        provider: "test".to_owned(),
        model: "scripted".to_owned(),
        wire: Wire::Chat,
        endpoint: Some("https://example.invalid/v1".to_owned()),
        api_key: ApiKey::new("test"),
        auth: None,
        no_prompt_cache_key: false,
        api_key_env: None,
        capabilities: capabilities(entry, Wire::Chat, None),
        headers: BTreeMap::new(),
    };
    let reference = ModelRef::new(
        ProviderId::new("test").expect("provider"),
        ModelId::new("scripted").expect("model"),
    );
    let model = Arc::new(
        ScriptedModel::new(reference)
            .turn_tool_call(
                CallId::new(),
                "write",
                &serde_json::json!({
                    "path": target.display().to_string(),
                    "content": "written after approval",
                    "overwrite": true
                }),
            )
            .turn_text("done"),
    );
    let service = Arc::new(
        Service::new(
            config,
            catalog,
            spec,
            Arc::new(ScriptedFactory {
                model: Arc::clone(&model) as Arc<dyn Model>,
            }),
            Arc::new(InMemoryStore::new()),
            Arc::new(InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    );

    let primary = service
        .submit(ClientRequest {
            request_id: 1,
            kind: ClientRequestKind::SendMessage {
                session: None,
                text: "write it".to_owned(),
                attachments: Vec::new(),
                budget: None,
                reasoning: None,
                replay: false,
            },
        })
        .await
        .expect("stream");

    let (approval_tx, mut approval_rx) = mpsc::unbounded_channel();
    let primary: EventStream = Box::pin(primary.inspect(move |event| {
        if matches!(&event.kind, ServerEventKind::ApprovalRequested { .. }) {
            let _ = approval_tx.send(());
        }
    }));
    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let mut input = crate::input::InteractiveInput::from_events(input_rx);
    let responder = tokio::spawn(async move {
        approval_rx.recv().await.expect("approval event");
        input_tx
            .send(crate::input::EditorEvent::Line("y".to_owned(), Vec::new()))
            .expect("approval input");
    });

    let mut renderer = Renderer::new(
        Format::Text,
        Verbosity::Low,
        false,
        Style::plain(),
        Pinned::new(),
    );
    let outcome = drive_turns(
        &service,
        primary,
        &mut renderer,
        Style::plain(),
        Some(&mut input),
    )
    .await
    .expect("drive");
    responder.await.expect("responder");

    assert!(!outcome.parked, "an approved turn must not stay parked");
    assert_eq!(model.remaining_turns(), 0, "the resumed turn must run");
    assert_eq!(
        std::fs::read_to_string(&target).expect("written file"),
        "written after approval"
    );
    let _ = std::fs::remove_file(&target);
    let _ = std::fs::remove_file(&key_file);
}
