#![allow(clippy::unwrap_used)]

use super::*;

#[test]
fn the_ui_block_is_a_subset_and_says_nothing_by_default() {
    // A config file that does not mention `ui` must behave exactly as it did
    // before the block existed, so every field has a default that is what
    // the client already did.
    let loaded = CliConfig::from_json("{}").expect("empty cli");
    let config = loaded.config;
    assert_eq!(config.verbosity, "medium");
    assert_eq!(config.diff_style, DiffStyle::Unified);
    assert_eq!(config.service_url, "http://127.0.0.1:8787");
    assert_eq!(config.service_type, ServiceType::Local);
    assert_eq!(config.service_auth, None);
}

#[cfg(test)]
mod service_config_tests {
    use super::*;

    #[test]
    fn service_json_selects_a_named_agent() {
        let service: ServiceConfig = serde_json::from_str(
            r#"{
                "version": 1,
                "default_agent": "reviewer",
                "agents": [
                    {"name": "builder", "description": null},
                    {"name": "reviewer", "author": "team"}
                ]
            }"#,
        )
        .expect("service");
        service.validate().expect("valid service");
        let agent = service.default_agent().expect("default");
        assert_eq!(agent.name, "reviewer");
        assert_eq!(agent.author.as_deref(), Some("team"));
    }

    #[test]
    fn service_json_rejects_duplicate_agent_names() {
        let error = serde_json::from_str::<ServiceConfig>(
            r#"{"default_agent":"same","agents":[{"name":"same"},{"name":"same"}]}"#,
        )
        .expect("parse")
        .validate()
        .expect_err("duplicate names must fail");
        assert!(error.to_string().contains("duplicate agent name"));
    }
}

#[test]
fn service_settings_are_client_only_and_preserve_raw_auth() {
    let loaded = CliConfig::from_json(
        r#"{"service_url":"https://agent.example/raep","service_type":"remote","service_auth":"raw-token"}"#,
    )
    .expect("service settings");
    assert_eq!(loaded.config.service_url, "https://agent.example/raep");
    assert_eq!(loaded.config.service_type, ServiceType::Remote);
    assert_eq!(loaded.config.service_auth.as_deref(), Some("raw-token"));
}

#[test]
fn the_diff_style_is_named_the_way_a_person_writes_it() {
    // The two names are what the configuration accepts, so they are part of
    // the interface and not a detail of the enum.
    let loaded = CliConfig::from_json(r#"{"diff_style":"split"}"#).expect("split");
    let config = loaded.config;
    assert_eq!(config.diff_style, DiffStyle::Split);
    let loaded = CliConfig::from_json(r#"{"diff_style":"unified"}"#).expect("unified");
    assert_eq!(loaded.config.diff_style, DiffStyle::Unified);
    // A name that is not one of the two is refused rather than silently
    // falling back, because a person who wrote `side-by-side` should be told.
    let error = CliConfig::from_json(r#"{"diff_style":"diagonal"}"#)
        .expect_err("an unknown style is refused");
    assert!(
        error.to_string().contains("diagonal"),
        "the error names the value: {error}"
    );
}

#[test]
fn verbosity_is_read_as_a_string_so_the_client_can_explain_a_bad_one() {
    // The enum lives in the client, not here: the engine never reads this
    // and a config crate that refused the value would make every host
    // reimplement the message.
    let loaded = CliConfig::from_json(r#"{"verbosity":"high"}"#).expect("high");
    let config = loaded.config;
    assert_eq!(config.verbosity, "high");
    let loaded = CliConfig::from_json(r#"{"verbosity":"loud"}"#).expect("any string");
    let config = loaded.config;
    assert_eq!(
        config.verbosity, "loud",
        "the client is what refuses it, with the levels it knows"
    );
}

#[test]
fn the_style_names_round_trip() {
    assert_eq!(DiffStyle::Unified.name(), "unified");
    assert_eq!(DiffStyle::Split.name(), "split");
    assert_eq!(DiffStyle::default(), DiffStyle::Unified);
}

#[test]
fn disabled_tools_are_read_from_the_file() {
    // The field as a person writes it, and the two cases that matter: the
    // list is read, and a file that does not mention it keeps every tool
    // rather than failing to load.
    let dir = std::env::temp_dir().join(format!("norte-disabled-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("config.json");
    std::fs::write(
        &path,
        r#"{"version":1,"tools":{"disabled":["shell","todo"]}}"#,
    )
    .expect("write");
    let loaded = Config::load(Some(&path), None).expect("load");
    assert_eq!(
        loaded.config.tools.disabled,
        vec!["shell".to_owned(), "todo".to_owned()],
        "the list is read from the file"
    );
    std::fs::write(&path, r#"{"version":1}"#).expect("write");
    let loaded = Config::load(Some(&path), None).expect("load");
    assert!(
        loaded.config.tools.disabled.is_empty(),
        "a file that does not mention it withholds nothing: {:?}",
        loaded.config.tools.disabled
    );
    let _ = std::fs::remove_dir_all(dir);
}
