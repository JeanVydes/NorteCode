use super::edit_source;

#[test]
fn edit_source_reads_only_regular_files_inside_the_workspace() {
    let root = std::env::temp_dir().join(format!("norte-edit-source-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).expect("temp root");
    let inside = root.join("note.txt");
    std::fs::write(&inside, "one\ntwo\n").expect("temp file");

    let arguments = serde_json::json!({"path": "note.txt"});
    assert_eq!(
        edit_source(&root, &arguments).as_deref(),
        Some("one\ntwo\n")
    );

    let outside =
        std::env::temp_dir().join(format!("norte-edit-outside-{}.txt", uuid::Uuid::new_v4()));
    std::fs::write(&outside, "secret").expect("outside file");
    let arguments = serde_json::json!({"path": outside});
    assert!(edit_source(&root, &arguments).is_none());

    let arguments = serde_json::json!({"path": ".."});
    assert!(edit_source(&root, &arguments).is_none());

    std::fs::remove_file(&outside).ok();
    std::fs::remove_dir_all(&root).ok();
}
