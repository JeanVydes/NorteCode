use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn temp_file(body: &str) -> PathBuf {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("norte-code-config-{}-{n}", std::process::id()));
    fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("config.json");
    fs::write(&path, body).expect("write");
    path
}

#[test]
fn defaults_are_valid() {
    let loaded = Config::from_json("{}").expect("config");
    assert_eq!(loaded.config.version, CONFIG_VERSION);
    assert_eq!(loaded.config.max_iterations, None);
    assert!(
        loaded.warnings.is_empty(),
        "a clean file warns about nothing"
    );
}

#[test]
fn a_file_over_the_limit_is_refused_before_parsing() {
    let path = temp_file("0123456789");
    let error = read_limited(&path, 4).expect_err("file must be bounded");
    assert!(error.to_string().contains("larger than 4 bytes"));
    let _ = fs::remove_dir_all(path.parent().expect("parent"));
}

#[test]
fn a_directory_is_not_a_regular_file() {
    let path = temp_file("{}");
    let error = read_limited(path.parent().expect("parent"), 64).expect_err("directory");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    let _ = fs::remove_dir_all(path.parent().expect("parent"));
}

#[test]
fn a_system_prompt_can_come_from_the_config() {
    let loaded = Config::from_json(r#"{"system_prompt":"Answer tersely."}"#).expect("config");
    assert_eq!(
        loaded.config.system_prompt.as_deref(),
        Some("Answer tersely.")
    );
    let defaults = Config::from_json("{}").expect("config");
    assert_eq!(defaults.config.system_prompt, None);
}

#[test]
fn vendor_extensions_are_typed_json() {
    let loaded =
        Config::from_json(r#"{"extensions":{"openai.chat:provider":{"order":["deepinfra"]}}}"#)
            .expect("config");
    assert_eq!(
        loaded.config.extensions["openai.chat:provider"],
        serde_json::json!({"order": ["deepinfra"]})
    );
}

#[test]
fn the_project_overlay_wins() {
    let user =
        temp_file(r#"{"default_provider":"openai","default_model":"gpt-5","max_iterations":4}"#);
    let project = temp_file(r#"{"max_iterations":8}"#);
    let loaded = Config::load(Some(&user), Some(&project)).expect("config");
    assert_eq!(loaded.config.max_iterations, Some(8));
    assert_eq!(loaded.config.default_provider.as_deref(), Some("openai"));
    assert_eq!(loaded.config.default_model.as_deref(), Some("gpt-5"));
}

#[test]
fn unknown_keys_warn_instead_of_failing() {
    let loaded = Config::from_json(r#"{"mystery":true}"#).expect("config");
    assert_eq!(loaded.warnings, vec!["mystery".to_owned()]);
}

#[test]
fn a_future_version_is_rejected() {
    let error = Config::from_json(r#"{"version":2}"#).expect_err("version");
    assert!(error.to_string().contains("version"));
}

#[test]
fn compaction_trigger_defaults_to_idle_and_rejects_unknown_modes() {
    let loaded = Config::from_json("{}").expect("config");
    assert_eq!(loaded.config.compaction.trigger, CompactionTrigger::Idle);
    let eager = Config::from_json(r#"{"compaction":{"trigger":"eager"}}"#).expect("config");
    assert_eq!(eager.config.compaction.trigger, CompactionTrigger::Eager);
    let error = Config::from_json(r#"{"compaction":{"trigger":"always"}}"#).expect_err("trigger");
    assert!(error.to_string().contains("always"));
}

#[test]
fn retry_defaults_are_bounded_and_capped_by_validation() {
    let loaded = Config::from_json("{}").expect("config");
    assert_eq!(loaded.config.retry.max_attempts, 4);
    assert_eq!(loaded.config.retry.max_delay_ms, 60_000);
    assert_eq!(loaded.config.retry.backoff_ms, 5_000);
    let zero = Config::from_json(r#"{"retry":{"max_attempts":0}}"#).expect("config");
    assert_eq!(zero.config.retry.max_attempts, 0);
    let error = Config::from_json(r#"{"retry":{"max_attempts":11}}"#).expect_err("max_attempts");
    assert!(error.to_string().contains("retry.max_attempts"));
    let error =
        Config::from_json(r#"{"retry":{"max_delay_ms":600001}}"#).expect_err("max_delay_ms");
    assert!(error.to_string().contains("retry.max_delay_ms"));
}

#[test]
fn command_model_bytes_default_and_validation() {
    let loaded = Config::from_json("{}").expect("config");
    assert_eq!(
        loaded.config.tools.command_max_model_bytes,
        DEFAULT_COMMAND_MAX_MODEL_BYTES
    );
    let custom =
        Config::from_json(r#"{"tools":{"command_max_model_bytes":65536}}"#).expect("custom limit");
    assert_eq!(custom.config.tools.command_max_model_bytes, 65_536);
    for value in ["0", "1023", "1048577"] {
        let error = Config::from_json(&format!(
            r#"{{"tools":{{"command_max_model_bytes":{value}}}}}"#
        ))
        .expect_err("out of range");
        assert!(error.to_string().contains("tools.command_max_model_bytes"));
    }
}

#[test]
fn confinement_confines_paths_and_leaves_the_sandbox_off() {
    let loaded = Config::from_json("{}").expect("config");
    assert!(
        loaded.config.tools.confinement.workspace_only,
        "tool paths are confined by default"
    );
    assert!(!loaded.config.tools.confinement.sandbox.enabled);
    assert!(!loaded.config.tools.confinement.sandbox.network);
    assert!(
        loaded.config.tools.confinement.sandbox.writable.is_empty(),
        "no writable path is granted by default"
    );

    let enabled = Config::from_json(
        r#"{"tools":{"confinement":{"sandbox":{"enabled":true,"network":true,"writable":["/tmp/cache"]}}}}"#,
    )
    .expect("config");
    assert!(enabled.config.tools.confinement.sandbox.enabled);
    assert!(enabled.config.tools.confinement.sandbox.network);
    assert_eq!(
        enabled.config.tools.confinement.sandbox.writable,
        vec![PathBuf::from("/tmp/cache")]
    );

    let open =
        Config::from_json(r#"{"tools":{"confinement":{"workspace_only":false}}}"#).expect("config");
    assert!(
        !open.config.tools.confinement.workspace_only,
        "the gate can be turned off without touching the sandbox"
    );
    assert!(!open.config.tools.confinement.sandbox.enabled);

    let error = Config::from_json(r#"{"tools":{"confinement":{"sandbox":{"writable":[""]}}}}"#)
        .expect_err("empty path");
    assert!(
        error
            .to_string()
            .contains("tools.confinement.sandbox.writable")
    );
}

#[test]
fn an_mcp_block_written_for_another_client_loads_unchanged() {
    // The shape Claude Desktop and Cursor write, copied verbatim.
    let loaded = Config::from_json(
        r#"{"mcpServers": {
            "filesystem": {
                "command": "npx",
                "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
                "env": { "TOKEN": "secret" }
            },
            "off": { "command": "slow", "enabled": false }
        }}"#,
    )
    .expect("the ecosystem format loads");

    let filesystem = loaded.config.mcp_servers.get("filesystem").expect("entry");
    assert_eq!(filesystem.command, "npx");
    assert_eq!(filesystem.args.len(), 3);
    assert_eq!(
        filesystem.env.get("TOKEN").map(String::as_str),
        Some("secret")
    );
    assert!(filesystem.enabled, "a server without `enabled` is on");
    assert!(
        !loaded.config.mcp_servers.get("off").expect("entry").enabled,
        "a disabled entry keeps its place"
    );
}

#[test]
fn a_remote_entry_is_a_url_and_not_a_command() {
    // The shape another client writes for a hosted server.
    let loaded = Config::from_json(
        r#"{"mcpServers":{"remote":{"url":"https://mcp.example.com/mcp","headers":{"Authorization":"Bearer x"}}}}"#,
    )
    .expect("a remote entry loads");
    let server = loaded.config.mcp_servers.get("remote").expect("entry");
    assert!(server.is_remote());
    assert_eq!(server.target(), "https://mcp.example.com/mcp");
    assert_eq!(
        server.headers.get("Authorization").map(String::as_str),
        Some("Bearer x")
    );

    // A local entry is labelled by its program.
    let local = Config::from_json(r#"{"mcpServers":{"local":{"command":"npx"}}}"#)
        .expect("a local entry loads")
        .config;
    let server = local.mcp_servers.get("local").expect("entry");
    assert!(!server.is_remote());
    assert_eq!(server.target(), "npx");

    // A url on its own is a remote server, which is the point of the form.
    Config::from_json(r#"{"mcpServers":{"x":{"url":"https://a"}}}"#)
        .expect("a url alone is a remote entry");

    // Nothing at all is a server that cannot be reached; both is ambiguous.
    let error =
        Config::from_json(r#"{"mcpServers":{"x":{}}}"#).expect_err("an empty entry is refused");
    assert!(error.to_string().contains("either"), "{error}");
    let error = Config::from_json(r#"{"mcpServers":{"x":{"command":"npx","url":"https://a"}}}"#)
        .expect_err("both is ambiguous");
    assert!(error.to_string().contains("cannot both"), "{error}");
}

#[test]
fn the_tool_lists_decide_what_reaches_the_model() {
    let published = ["echo", "get-sum", "get-env"];

    // Nothing named: every tool the server publishes.
    let open = McpServerConfig::default();
    assert_eq!(open.select(published.iter()).len(), 3);

    // An allow list is the whole of it.
    let only = McpServerConfig {
        allow: vec!["echo".to_owned(), "get-sum".to_owned()],
        ..McpServerConfig::default()
    };
    let kept = only.select(published.iter());
    assert_eq!(kept.len(), 2, "{kept:?}");
    assert!(kept.iter().any(|tool| **tool == "echo"), "{kept:?}");
    assert!(
        !kept.iter().any(|tool| **tool == "get-env"),
        "a tool outside the allow list is absent: {kept:?}"
    );

    // A deny list takes away from everything else.
    let without = McpServerConfig {
        deny: vec!["get-env".to_owned()],
        ..McpServerConfig::default()
    };
    let kept = without.select(published.iter());
    assert_eq!(kept.len(), 2, "{kept:?}");
    assert!(!kept.iter().any(|tool| **tool == "get-env"), "{kept:?}");

    // Deny wins over allow, so naming a tool in both keeps it out.
    let both = McpServerConfig {
        allow: vec!["echo".to_owned()],
        deny: vec!["echo".to_owned()],
        ..McpServerConfig::default()
    };
    assert!(
        both.select(published.iter()).is_empty(),
        "neither selector matches a tool outside its list"
    );
}

#[test]
fn mcp_entries_are_validated() {
    // A server that is on must name a program to run.
    let error = Config::from_json(r#"{"mcpServers":{"x":{"command":"  "}}}"#)
        .expect_err("an empty command is refused");
    assert!(error.to_string().contains("mcpServers.x"), "{error}");
    assert!(error.to_string().contains("command"), "{error}");

    // A disabled entry may be incomplete: it is off, so it is not started.
    Config::from_json(r#"{"mcpServers":{"x":{"command":"","enabled":false}}}"#)
        .expect("a disabled placeholder is fine");

    // The name becomes part of every tool name it exposes.
    let error = Config::from_json(r#"{"mcpServers":{"a__b":{"command":"npx"}}}"#)
        .expect_err("the namespace separator is refused");
    assert!(error.to_string().contains("a__b"), "{error}");
    let error = Config::from_json(r#"{"mcpServers":{"a b":{"command":"npx"}}}"#)
        .expect_err("a space is refused");
    assert!(error.to_string().contains("a b"), "{error}");
}
#[test]
fn a_bad_budget_is_rejected() {
    let error =
        Config::from_json(r#"{"budgets":{"session":{"unit":"usd","limit":0,"action":"stop"}}}"#)
            .expect_err("budget");
    assert!(error.to_string().contains("budgets.session.limit"));
}

#[test]
fn a_global_approval_rule_moves_between_lists_and_keeps_unknown_keys() {
    let path = temp_file(r#"{"mystery":true}"#);
    update_user_approval(&path, "command", true).expect("allow");
    update_user_approval(&path, "command", true).expect("allow again");
    let body = fs::read_to_string(&path).expect("read");
    let root: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(root["approvals"]["allow"], serde_json::json!(["command"]));
    assert_eq!(root["mystery"], true);

    update_user_approval(&path, "command", false).expect("deny");
    let loaded = Config::load(Some(&path), None).expect("config");
    assert!(
        loaded.config.approvals.allow.is_empty(),
        "a deny rule is not an allow"
    );
    assert_eq!(loaded.config.approvals.deny, ["command"]);
    let leftovers = fs::read_dir(path.parent().expect("parent"))
        .expect("directory")
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp"))
        .count();
    assert_eq!(leftovers, 0);
}

#[test]
fn an_mcp_server_is_added_turned_off_and_removed() {
    // The three writes the `mcp` command makes, against one file, keeping
    // what else was in it.
    let path = temp_file(r#"{"mystery":true}"#);
    add_user_mcp_server(
        &path,
        "context7",
        "npx",
        &["-y".to_owned(), "@upstash/context7-mcp".to_owned()],
    )
    .expect("add");
    let root: Value =
        serde_json::from_str(&fs::read_to_string(&path).expect("read")).expect("json");
    assert_eq!(root["mcpServers"]["context7"]["command"], "npx");
    assert_eq!(
        root["mcpServers"]["context7"]["args"],
        serde_json::json!(["-y", "@upstash/context7-mcp"])
    );
    assert_eq!(root["mcpServers"]["context7"]["enabled"], true);
    assert_eq!(root["mystery"], true, "the rest of the file survives");

    set_user_mcp_enabled(&path, "context7", false).expect("disable");
    let loaded = Config::load(Some(&path), None).expect("config");
    assert!(
        !loaded
            .config
            .mcp_servers
            .get("context7")
            .expect("entry")
            .enabled
    );

    set_user_mcp_enabled(&path, "context7", true).expect("enable");
    let loaded = Config::load(Some(&path), None).expect("config");
    assert!(
        loaded
            .config
            .mcp_servers
            .get("context7")
            .expect("entry")
            .enabled
    );

    remove_user_mcp_server(&path, "context7").expect("remove");
    let loaded = Config::load(Some(&path), None).expect("config");
    assert!(loaded.config.mcp_servers.is_empty(), "the entry is gone");

    // Removing one that is not there is not an error.
    remove_user_mcp_server(&path, "context7").expect("remove again");
    let _ = fs::remove_dir_all(path.parent().expect("parent"));
}

#[test]
fn an_mcp_write_needs_a_name_and_a_command() {
    let path = temp_file("{}");
    assert!(add_user_mcp_server(&path, "  ", "npx", &[]).is_err());
    assert!(add_user_mcp_server(&path, "x", "  ", &[]).is_err());
    assert!(set_user_mcp_enabled(&path, "  ", false).is_err());
    // A server the file does not name cannot be toggled: the mistake is the
    // name, and saying so is what the person needs.
    assert!(set_user_mcp_enabled(&path, "nope", false).is_err());
    let _ = fs::remove_dir_all(path.parent().expect("parent"));
}

#[test]
fn forgetting_a_rule_removes_it_from_both_lists() {
    // What "ask again" writes: the tool is gone from the file, so the next
    // start does not read a rule back. Both lists are touched, because a
    // tool with an allow and a deny is a tool somebody changed their mind
    // about.
    let path = temp_file("{}");
    update_user_approval(&path, "command", true).expect("allow");
    forget_user_approval(&path, "command").expect("forget");
    let loaded = Config::load(Some(&path), None).expect("config");
    assert!(
        loaded.config.approvals.allow.is_empty(),
        "the allow is gone"
    );
    assert!(
        loaded.config.approvals.deny.is_empty(),
        "and no deny either"
    );

    // A deny that is forgotten goes the same way.
    update_user_approval(&path, "write", false).expect("deny");
    forget_user_approval(&path, "write").expect("forget");
    let loaded = Config::load(Some(&path), None).expect("config");
    assert!(
        loaded.config.approvals.deny.is_empty(),
        "the deny is gone too"
    );

    // A tool that is not there is not an error: the key is idempotent, so a
    // second press is not the panel arguing with the person.
    forget_user_approval(&path, "write").expect("forget a rule that is not there");
    let _ = fs::remove_dir_all(path.parent().expect("parent"));
}

#[test]
fn forgetting_a_rule_needs_a_tool_name() {
    let path = temp_file("{}");
    assert!(forget_user_approval(&path, "  ").is_err());
    let _ = fs::remove_dir_all(path.parent().expect("parent"));
}

#[cfg(unix)]
#[test]
fn a_symlinked_config_stays_a_symlink() {
    use std::os::unix::fs::symlink;

    let target = temp_file("{}");
    let directory = target.parent().expect("parent").join("link");
    fs::create_dir_all(&directory).expect("directory");
    let link = directory.join("config.json");
    symlink(&target, &link).expect("symlink");
    update_user_approval(&link, "command", true).expect("update");
    assert!(
        fs::symlink_metadata(&link)
            .expect("metadata")
            .file_type()
            .is_symlink()
    );
    let body = fs::read_to_string(&target).expect("target");
    assert!(body.contains("\"command\""));
    let _ = fs::remove_dir_all(target.parent().expect("parent"));
}

#[cfg(unix)]
#[test]
fn a_new_config_is_private() {
    use std::os::unix::fs::PermissionsExt;

    let path = temp_file("{}");
    fs::remove_file(&path).expect("remove");
    update_user_approval(&path, "command", true).expect("update");
    let mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let _ = fs::remove_dir_all(path.parent().expect("parent"));
}

#[test]
fn a_key_comes_from_the_first_variable_that_has_one() {
    // The order is the catalogue's: a provider lists its variables and the
    // first one that is set wins. The variable read here is one the harness
    // exports for the test run, so nothing mutates the environment of a
    // process other tests share.
    let loaded = Config::from_json("{}").expect("config");
    let present = "NORTE_CODE_TEST_PRESENT_KEY";
    if std::env::var(present).is_err() {
        // The variable is set by `build.rs`-less means: this test is
        // skipped rather than made to depend on how it was invoked.
        eprintln!("skipping: {present} is not set in this environment");
        return;
    }
    let key = loaded
        .config
        .resolve_api_key(
            "test",
            &["NORTE_CODE_TEST_ABSENT_KEY".to_owned(), present.to_owned()],
        )
        .expect("key");
    assert_eq!(key.expose().trim(), key.expose(), "trimmed");
    assert_ne!(key.expose().len(), 0, "a value was found");
}

#[test]
fn a_provider_with_no_variable_needs_no_key() {
    // A local endpoint names nothing, and that is not a failure: there is
    // nothing to look up, so there is nothing to be missing.
    let loaded = Config::from_json("{}").expect("config");
    let key = loaded.config.resolve_api_key("local", &[]).expect("key");
    assert_eq!(key.expose(), "", "nothing to send");
}

#[test]
fn a_missing_key_names_what_was_tried() {
    let loaded = Config::from_json("{}").expect("config");
    let error = loaded
        .config
        .resolve_api_key("nobody", &["NORTE_CODE_TEST_NOBODY_API_KEY".to_owned()])
        .expect_err("key");
    assert!(error.to_string().contains("NORTE_CODE_TEST_NOBODY_API_KEY"));
}

#[derive(Debug)]
struct FakeKeys;

impl crate::keys::KeyStore for FakeKeys {
    fn resolve(&self, provider: &str, _environment_keys: &[String]) -> Result<ApiKey, ConfigError> {
        Ok(ApiKey::new(format!("{provider}-key")))
    }

    fn known(&self, provider: &str, _environment_keys: &[String]) -> Option<ApiKey> {
        Some(ApiKey::new(format!("{provider}-key")))
    }

    fn save(
        &self,
        _provider: &str,
        _key: &str,
    ) -> Result<crate::keys::KeyStorage, crate::keys::KeyStoreError> {
        Ok(crate::keys::KeyStorage::File)
    }

    fn delete(&self, _provider: &str) -> Result<(), crate::keys::KeyStoreError> {
        Ok(())
    }
}

#[test]
fn a_host_store_answers_and_is_never_serialized() {
    let config = Config {
        keys: Some(Arc::new(FakeKeys)),
        ..Config::default()
    };
    let key = config
        .resolve_api_key("ollamacloud", &["NORTE_CODE_TEST_ABSENT_KEY".to_owned()])
        .expect("key");
    assert_eq!(key.expose(), "ollamacloud-key");
    assert_eq!(
        config
            .known_api_key("ollamacloud", &[])
            .expect("known")
            .expose(),
        "ollamacloud-key"
    );
    let value = serde_json::to_value(&config).expect("serialize");
    assert!(value.get("keys").is_none(), "the store is not a file field");
}
