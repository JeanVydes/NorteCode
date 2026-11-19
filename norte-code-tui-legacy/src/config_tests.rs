use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use super::{ChartsConfig, ClientConfig, DotSize, LegacyConfig, ServiceType};
use crate::{
    tools::DiffMode,
    ui_model::{ThemeMode, ThinkingVisibility, Toggle, UiModel},
};
use norte_code_wire::BudgetAction;

#[test]
fn migrates_legacy_cli_settings_into_client_and_cli_sections() {
    let directory = temporary_directory();
    fs::create_dir_all(&directory).expect("create fixture directory");
    let client_path = directory.join("client.json");
    let legacy_path = directory.join("cli.json");
    let legacy = LegacyConfig {
        service_url: Some("http://127.0.0.1:8787".to_owned()),
        service_type: Some(ServiceType::Local),
        service_token: Some("kept-in-config".to_owned()),
        agent_id: Some("norte-code".to_owned()),
        developer_mode: Some(true),
        diff_style: Some(DiffMode::Unified),
        charts: Some(ChartsConfig {
            contrast: 75,
            density: 82,
            dot_size: DotSize::Medium,
            ..ChartsConfig::default()
        }),
    };
    fs::write(
        &legacy_path,
        serde_json::to_vec(&legacy).expect("serialize fixture"),
    )
    .expect("write fixture");

    let config =
        ClientConfig::load_from_paths(&client_path, &legacy_path).expect("migrate configuration");

    assert_eq!(config.service.url, "http://127.0.0.1:8787");
    assert_eq!(config.service.token.as_deref(), Some("kept-in-config"));
    assert!(config.preferences.developer_mode.is_on());
    assert_eq!(config.budget.action, BudgetAction::Ask);
    assert_eq!(config.cli.charts.contrast, 75);
    assert_eq!(config.cli.charts.density, 82);
    assert!(matches!(config.cli.charts.dot_size, DotSize::Medium));
    assert!(client_path.exists());
    assert!(!legacy_path.exists());

    let migrated: serde_json::Value =
        serde_json::from_slice(&fs::read(&client_path).expect("read migrated config"))
            .expect("parse migrated config");
    assert_eq!(migrated["version"], 1);
    assert_eq!(migrated["service"]["url"], "http://127.0.0.1:8787");
    assert_eq!(migrated["preferences"]["developer_mode"], true);
    assert_eq!(migrated["budget"]["action"], "ask");
    assert_eq!(migrated["cli"]["charts"]["contrast"], 75);
    assert!(migrated.get("charts").is_none());
    fs::remove_dir_all(directory).expect("remove fixture directory");
}

#[test]
fn loads_and_saves_tui_preferences_without_losing_other_client_settings() {
    let directory = temporary_directory();
    fs::create_dir_all(&directory).expect("create fixture directory");
    let path = directory.join("client.json");
    let mut config = ClientConfig {
        path: path.clone(),
        ..ClientConfig::default()
    };
    config.cli.charts.contrast = 91;
    config.preferences.theme = ThemeMode::Light;
    config.preferences.thinking_visibility = ThinkingVisibility::Expanded;
    config.save().expect("save default configuration");

    let mut model = UiModel::default();
    let loaded = ClientConfig::read(&path).expect("load saved configuration");
    loaded.apply_to(&mut model);
    assert_eq!(model.preferences.theme, ThemeMode::Light);
    assert_eq!(model.preferences.thinking, ThinkingVisibility::Expanded);
    assert_eq!(model.preferences.show_timestamps, Toggle::On);
    assert_eq!(model.cli.diff_style, DiffMode::Unified);

    model.cli.diff_style = DiffMode::Split;
    model.preferences.developer_mode = Toggle::On;
    model.budget_action = BudgetAction::Stop;
    let mut updated = loaded;
    updated.update_preferences(&model);
    updated.save().expect("persist preference changes");
    let persisted = ClientConfig::read(&path).expect("reload preferences");
    assert_eq!(persisted.cli.diff_style, DiffMode::Split);
    assert!(persisted.preferences.developer_mode.is_on());
    assert_eq!(persisted.budget.action, BudgetAction::Stop);
    assert_eq!(persisted.cli.charts.contrast, 91);
    fs::remove_dir_all(directory).expect("remove fixture directory");
}

fn temporary_directory() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    Path::new(&std::env::temp_dir())
        .join(format!("norte-code-config-{}-{nonce}", std::process::id()))
}
