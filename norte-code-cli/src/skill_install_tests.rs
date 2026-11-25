use std::fs;

use super::{DIRECTORY, MARKER, sync_at};

#[test]
fn managed_skill_is_created_preserved_and_removed() {
    let root = std::env::temp_dir().join(format!("norte-skill-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    sync_at(&root, true).expect("install");
    let dir = root.join(DIRECTORY);
    assert!(dir.join(MARKER).is_file());
    let registry = norte_code_protocol::skills::SkillRegistry::discover(
        std::slice::from_ref(&root),
        10,
        norte_code_protocol::config::SkillLimitsConfig::default(),
    );
    assert!(
        registry.get("norte-code").is_some(),
        "{:?}",
        registry.warnings()
    );
    fs::write(dir.join("SKILL.md"), "customized").expect("edit");
    sync_at(&root, true).expect("keep existing");
    assert_eq!(
        fs::read_to_string(dir.join("SKILL.md")).expect("read"),
        "customized"
    );
    sync_at(&root, false).expect("remove");
    assert!(!dir.exists());
    fs::create_dir_all(&dir).expect("foreign directory");
    fs::write(dir.join("SKILL.md"), "foreign").expect("foreign skill");
    sync_at(&root, false).expect("leave foreign skill");
    assert!(dir.join("SKILL.md").exists());
    fs::remove_dir_all(root).expect("cleanup");
}
