use super::{
    Cli, Command, McpCommand, SandboxOptions, Style, apply_extensions, budget_label,
    budget_warning, compose_selection, cost_summary, parse_reasoning_setting, select_system_prompt,
};
use crate::commands::{RequestSummary, bucket_label, percentile, telemetry_table};
use crate::preview::{PreviewLine, argument_preview};
use chrono::{DateTime, Utc};
use clap::Parser;
use norte_code_protocol::catalog::WireSchema;
use norte_code_protocol::config::{BudgetConfig, Config};
use norte_code_protocol::costs::CostSummary;
use norte_code_protocol::sqlite::{RequestRecord, RequestStatus};
use norte_code_protocol::{Budget, BudgetAction, BudgetUnit};

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
        status: RequestStatus::Ok,
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
    let mut config = norte_code_protocol::config::Config::default();
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
    // The schema is the one `providers.json` writes, so a person who read the
    // file knows what to type. `openai` is the chat completions surface: the
    // Responses API exists in Norte and is deliberately not reachable here.
    let cli = Cli::try_parse_from(["norte-code", "new", "--wire", "openai"]).expect("parse");
    let Some(Command::New(args)) = cli.command else {
        panic!("expected new");
    };
    assert_eq!(args.wire, Some(WireSchema::OpenAI));

    let cli = Cli::try_parse_from(["norte-code", "new", "--wire", "anthropic"]).expect("parse");
    let Some(Command::New(args)) = cli.command else {
        panic!("expected new");
    };
    assert_eq!(args.wire, Some(WireSchema::Anthropic));

    // A name that is not a schema is refused where it is typed, and the message
    // names the three that exist.
    // clap reports a bad `value_parser` as a parse error, and `Cli` is not
    // `Debug`, so the error is read from the result by hand.
    match Cli::try_parse_from(["norte-code", "new", "--wire", "telepathy"]) {
        Ok(_) => panic!("an unknown wire must be refused"),
        Err(error) => {
            let message = error.to_string();
            assert!(message.contains("telepathy"), "{message}");
            assert!(message.contains("anthropic"), "{message}");
        }
    }
}

#[test]
fn the_mcp_command_parses_add_and_the_switch() {
    // `add` takes a name, a program and everything after it, hyphenated flags
    // included: an MCP server's arguments are the program's, not ours.
    let cli = Cli::try_parse_from([
        "norte-code",
        "mcp",
        "add",
        "context7",
        "npx",
        "-y",
        "@upstash/context7-mcp",
    ])
    .expect("add parses");
    let Some(Command::Mcp(args)) = cli.command else {
        panic!("expected mcp");
    };
    let McpCommand::Add {
        name,
        command,
        args,
    } = args.command
    else {
        panic!("expected add");
    };
    assert_eq!(name, "context7");
    assert_eq!(command, "npx");
    assert_eq!(args, ["-y", "@upstash/context7-mcp"]);

    let cli = Cli::try_parse_from(["norte-code", "mcp", "disable", "context7"]).expect("disable");
    let Some(Command::Mcp(args)) = cli.command else {
        panic!("expected mcp");
    };
    assert!(matches!(args.command, McpCommand::Disable { name } if name == "context7"));

    // A subcommand is required: `norte-code mcp` alone has nothing to do.
    assert!(Cli::try_parse_from(["norte-code", "mcp"]).is_err());
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
fn the_percentiles_of_one_request_are_that_request() {
    // A percentile of one thing is that thing at every level: the index rounds
    // down and clamps, so a single request is its own p50, p75 and p95. Without
    // the clamp, a request count of one indexes past the end.
    let mut values = vec![120u64];
    assert_eq!(percentile(&mut values, 50), 120);
    assert_eq!(percentile(&mut values, 75), 120);
    assert_eq!(percentile(&mut values, 95), 120);
    // An empty set has no percentile, and zero is what "nothing was measured"
    // prints as.
    assert_eq!(percentile(&mut [], 95), 0);
}

#[test]
fn a_percentile_reads_the_sorted_set_and_not_the_first_one() {
    // The three say different things about the same set, which is the whole
    // reason for having three: the middle request, the slow three in four, and
    // the worst in twenty. A mean over these is dragged by the last two.
    // The index is `len * p / 100`, so over `1..=100` the p50 is the value at
    // index 50 — which is 51, because the index counts from zero.
    let mut values: Vec<u64> = (1..=100).collect();
    assert_eq!(percentile(&mut values, 50), 51, "the middle one");
    assert_eq!(percentile(&mut values, 75), 76);
    assert_eq!(percentile(&mut values, 95), 96, "the worst in twenty");
    // And it reads the values, not the positions: an unsorted set gives the
    // same answer because the function sorts it.
    let mut shuffled = vec![95u64, 1, 50, 75, 20];
    assert_eq!(percentile(&mut shuffled, 50), 50);
    assert_eq!(percentile(&mut shuffled, 75), 75);
    assert_eq!(percentile(&mut shuffled, 95), 95);
}

#[test]
fn the_telemetry_row_carries_the_percentiles_the_average_hides() {
    // The columns exist because a mean says nothing about the request a person
    // is waiting on: one slow call in twenty moves the average and the p95 by
    // different amounts, and the table has to show both.
    let mut summary = RequestSummary::default();
    for _ in 0..19 {
        summary.add(&telemetry_record("openai", "gpt-5", Some(300), None));
    }
    // One slow call in twenty: the mean moves a little, the p95 moves to it.
    let mut slow = telemetry_record("openai", "gpt-5", Some(300), None);
    slow.duration_ms = 60_000;
    summary.add(&slow);

    let mut timing = summary.clone();
    let p50 = timing.p50_ms();
    let p95 = timing.p95_ms();
    assert_eq!(p50, 1_000, "every ordinary request took the same time");
    assert_eq!(p95, 60_000, "and the p95 is the slow one");
    assert!(
        summary.average_ms() < p95 / 2,
        "one slow call in twenty moves the mean far less than the p95: {} vs {p95}",
        summary.average_ms()
    );
}
