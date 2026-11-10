use super::*;

#[test]
fn plugin_dir_defaults_to_home_config() {
    let dir = plugin_dir();
    assert!(dir.to_string_lossy().contains(".config/norte-code/plugins"));
}

#[test]
fn list_plugins_returns_only_library_stems() {
    let names = list_plugins();
    assert!(names.iter().all(|name| !name.is_empty()));
}

#[test]
fn a_workspace_requires_a_package_when_it_contains_multiple_plugins() {
    let metadata = serde_json::json!({
        "packages": [
            {"id": "path+file:///repo#first@0.1.0", "name": "first", "targets": [
                {"name": "first", "crate_types": ["cdylib"]}
            ]},
            {"id": "path+file:///repo#second@0.1.0", "name": "second", "targets": [
                {"name": "second", "crate_types": ["cdylib"]}
            ]},
            {"id": "path+file:///repo#helper@0.1.0", "name": "helper", "targets": [
                {"name": "helper", "crate_types": ["lib"]}
            ]}
        ]
    });
    let packages = plugin_packages(&metadata).expect("workspace packages");
    let error = select_plugin_package(&packages, None).expect_err("ambiguous workspace");
    assert!(error.contains("first, second"), "{error}");

    let selected = select_plugin_package(&packages, Some("second")).expect("selected package");
    assert_eq!(selected.name, "second");
    assert_eq!(selected.id, "path+file:///repo#second@0.1.0");
}

#[test]
fn a_single_workspace_plugin_is_selected_without_a_package_name() {
    let metadata = serde_json::json!({
        "packages": [
            {"id": "path+file:///repo#plugin@0.1.0", "name": "plugin", "targets": [
                {"name": "plugin", "crate_types": ["cdylib"]}
            ]},
            {"id": "path+file:///repo#helper@0.1.0", "name": "helper", "targets": [
                {"name": "helper", "crate_types": ["lib"]}
            ]}
        ]
    });
    let packages = plugin_packages(&metadata).expect("workspace packages");
    assert_eq!(
        select_plugin_package(&packages, None)
            .expect("unique plugin")
            .name,
        "plugin"
    );
}

#[test]
fn recorded_sources_keep_legacy_urls_and_workspace_packages() {
    let sources = parse_sources(
        r#"{"old":"https://example.invalid/old","new":{"url":"https://example.invalid/repo","package":"new-plugin"}}"#,
    )
    .expect("sources");
    assert_eq!(sources["old"].url, "https://example.invalid/old");
    assert_eq!(sources["old"].package, None);
    assert_eq!(sources["new"].package.as_deref(), Some("new-plugin"));
}

#[test]
fn the_cdylib_is_read_out_of_the_cargo_messages() {
    // The directory holds every dependency's library too, and one of
    // those is not a plugin.
    let messages = br#"{"reason":"compiler-artifact","package_id":"path+file:///repo#plugin@0.1.0","target":{"kind":["cdylib"]},"filenames":["/tmp/target/release/libplugin.so"]}
{"reason":"compiler-artifact","package_id":"path+file:///repo#helper@0.1.0","target":{"kind":["cdylib"]},"filenames":["/tmp/target/release/libhelper.so"]}
{"reason":"compiler-artifact","package_id":"path+file:///repo#plugin@0.1.0","target":{"kind":["lib"]},"filenames":["/tmp/target/release/libplugin.rlib"]}
"#;
    assert_eq!(
        cdylib(messages, "path+file:///repo#plugin@0.1.0"),
        Some(PathBuf::from("/tmp/target/release/libplugin.so"))
    );
}

#[test]
fn a_build_without_a_cdylib_has_no_library() {
    let messages = br#"{"reason":"compiler-artifact","package_id":"path+file:///repo#plugin@0.1.0","target":{"kind":["lib"]},"filenames":["/tmp/target/release/libplugin.rlib"]}
"#;
    assert_eq!(cdylib(messages, "path+file:///repo#plugin@0.1.0"), None);
}

#[test]
fn the_error_keeps_the_last_lines_and_not_the_whole_log() {
    let output = b"one\ntwo\nthree\nfour\nfive\n";
    assert_eq!(tail(output, 2), "four\nfive");
}

#[test]
fn an_error_without_output_still_says_something() {
    assert_eq!(tail(b"", 4), "no output");
}
