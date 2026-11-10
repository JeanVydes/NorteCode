#![allow(clippy::wildcard_imports)]
use super::*;

fn write_skill(root: &Path, name: &str, frontmatter: &str, body: &str) -> PathBuf {
    let directory = root.join(name);
    std::fs::create_dir_all(&directory).expect("skill dir");
    std::fs::write(
        directory.join("SKILL.md"),
        format!("---\n{frontmatter}\n---\n{body}"),
    )
    .expect("SKILL.md");
    directory
}

fn temp_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("norte-code-skills-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("temp root");
    root
}

#[test]
fn untrusted_skill_text_is_escaped() {
    assert_eq!(escape("a\u{202e}b\x1b[2J"), "a\\u{202e}b\\u{1b}[2J");
    assert_eq!(escape("a\n\tb"), "a\n\tb");
}

#[test]
fn valid_names_follow_the_specification() {
    for name in ["pdf-processing", "data-analysis", "code-review", "a1"] {
        assert!(validate_name(name, Some(name)).is_ok(), "{name}");
    }
    for name in [
        "PDF-Processing",
        "-pdf",
        "pdf-",
        "pdf--processing",
        "pdf_processing",
    ] {
        assert!(validate_name(name, Some(name)).is_err(), "{name}");
    }
    assert!(validate_name("pdf", Some("other")).is_err());
}

#[test]
fn a_valid_skill_is_parsed() {
    let skill = validation::parse_skill(
        "---\nname: pdf-processing\ndescription: Extract PDF text and forms. Use for PDFs.\nlicense: Apache-2.0\nmetadata:\n  author: example\nallowed-tools: Bash(git:*) Read\n---\n# Body\n",
        "pdf-processing",
    )
    .expect("skill");
    assert_eq!(skill.name, "pdf-processing");
    assert_eq!(skill.allowed_tools, ["Bash(git:*)", "Read"]);
    assert_eq!(
        skill.metadata.get("author").map(String::as_str),
        Some("example")
    );
    assert!(skill.body.contains("# Body"));
}

#[test]
fn malformed_skills_are_skipped_with_a_warning() {
    let root = temp_root("warn");
    write_skill(
        &root,
        "good",
        "name: good\ndescription: A good skill.",
        "body",
    );
    write_skill(&root, "Bad-Name", "name: Bad-Name\ndescription: x", "body");
    write_skill(&root, "empty", "name: empty\ndescription: \"\"", "body");
    let registry = SkillRegistry::discover(std::slice::from_ref(&root), 16);
    assert!(registry.get("good").is_some());
    assert_eq!(registry.len(), 1);
    assert_eq!(registry.warnings().len(), 2);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_later_root_shadows_an_earlier_one() {
    let user = temp_root("user");
    let project = temp_root("project");
    write_skill(&user, "same", "name: same\ndescription: user", "user body");
    write_skill(
        &project,
        "same",
        "name: same\ndescription: project",
        "project body",
    );
    let registry = SkillRegistry::discover(&[user.clone(), project.clone()], 16);
    assert_eq!(
        registry.get("same").map(|skill| skill.description.as_str()),
        Some("project")
    );
    assert!(
        registry
            .warnings()
            .iter()
            .any(|warning| warning.message.contains("shadows"))
    );
    let _ = std::fs::remove_dir_all(user);
    let _ = std::fs::remove_dir_all(project);
}

#[test]
fn the_prompt_order_puts_the_most_specific_root_first() {
    // The shape this exists for: twenty-five user-wide skills whose names
    // sort early, and one workspace skill whose name sorts late. A name
    // sort gives the budget to the AWS set and leaves the workspace's own
    // skill out.
    let user = temp_root("precedence-user");
    let project = temp_root("precedence-project");
    write_skill(
        &user,
        "amazon-bedrock",
        "name: amazon-bedrock\ndescription: aws",
        "b",
    );
    write_skill(
        &user,
        "aws-compute",
        "name: aws-compute\ndescription: aws",
        "b",
    );
    write_skill(
        &project,
        "rust-best-practices",
        "name: rust-best-practices\ndescription: rust",
        "b",
    );

    // The workspace root is last, which makes it the most specific.
    let registry = SkillRegistry::discover(&[user.clone(), project.clone()], 16);
    let ordered: Vec<&str> = registry
        .skills_by_precedence()
        .iter()
        .map(|skill| skill.name.as_str())
        .collect();
    assert_eq!(
        ordered,
        ["rust-best-practices", "amazon-bedrock", "aws-compute"],
        "the workspace skill comes first, the rest by name inside their root"
    );
    // And the alphabetical view is still available for a listing.
    let by_name: Vec<&str> = registry
        .skills()
        .iter()
        .map(|skill| skill.name.as_str())
        .collect();
    assert_eq!(by_name[0], "amazon-bedrock", "{by_name:?}");

    let _ = std::fs::remove_dir_all(user);
    let _ = std::fs::remove_dir_all(project);
}

#[test]
fn the_prompt_budget_favours_the_workspace_skill() {
    // The point of the order: with room for one, the workspace skill is the
    // one that reaches the prompt.
    let user = temp_root("budget-user");
    let project = temp_root("budget-project");
    for name in ["amazon-bedrock", "aws-compute", "aws-storage"] {
        write_skill(
            &user,
            name,
            &format!(
                "name: {name}\ndescription: a long winded AWS description that fills the budget"
            ),
            "b",
        );
    }
    write_skill(
        &project,
        "zzz-workspace",
        "name: zzz-workspace\ndescription: the one this repo is for",
        "b",
    );

    let registry = SkillRegistry::discover(&[user.clone(), project.clone()], 16);
    let index: Vec<(String, String)> = registry
        .skills_by_precedence()
        .iter()
        .map(|skill| (skill.name.clone(), skill.description.clone()))
        .collect();
    assert_eq!(
        index[0].0, "zzz-workspace",
        "the workspace skill leads the index the prompt is built from"
    );

    let _ = std::fs::remove_dir_all(user);
    let _ = std::fs::remove_dir_all(project);
}

#[test]
fn resources_cannot_escape_the_skill_root() {
    let root = temp_root("resources");
    let directory = write_skill(
        &root,
        "reader",
        "name: reader\ndescription: Reads things.",
        "body",
    );
    std::fs::create_dir_all(directory.join("references")).expect("references");
    std::fs::write(directory.join("references/REFERENCE.md"), "hello").expect("reference");
    std::fs::write(root.join("secret.txt"), "secret").expect("secret");
    let skill = load_skill(&directory).expect("skill").0;
    assert_eq!(
        skill
            .read_resource("references/REFERENCE.md", 1024)
            .expect("read"),
        "hello"
    );
    assert!(skill.read_resource("../secret.txt", 1024).is_err());
    assert!(skill.read_resource("/etc/passwd", 1024).is_err());
    let _ = std::fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn a_skill_file_cannot_symlink_outside_the_skill_root() {
    use std::os::unix::fs::symlink;

    let root = temp_root("symlink");
    let outside = temp_root("symlink-outside");
    std::fs::write(
        outside.join("SKILL.md"),
        "---\nname: escaped\ndescription: Outside the root.\n---\nsecret",
    )
    .expect("outside skill");
    let directory = root.join("escaped");
    std::fs::create_dir_all(&directory).expect("skill dir");
    symlink(outside.join("SKILL.md"), directory.join("SKILL.md")).expect("symlink");

    assert!(load_skill(&directory).is_err());
    let registry = SkillRegistry::discover(std::slice::from_ref(&root), 8);
    assert!(registry.is_empty());
    assert!(
        registry
            .warnings()
            .iter()
            .any(|warning| warning.message.contains("leaves the skill directory"))
    );
    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(outside);
}

#[test]
fn a_missing_closing_fence_is_an_error() {
    assert!(validation::parse_skill("---\nname: x\n", "x").is_err());
    assert!(validation::parse_skill("no frontmatter", "x").is_err());
}

#[tokio::test]
async fn a_script_runs_without_a_shell_and_with_argv_entries() {
    use norte::FnExecutor as _;

    let root = temp_root("script");
    let directory = write_skill(
        &root,
        "runner",
        "name: runner\ndescription: Runs a script.",
        "body",
    );
    std::fs::create_dir_all(directory.join("scripts")).expect("scripts");
    std::fs::write(
        directory.join("scripts/echo.sh"),
        "#!/bin/sh\nprintf '%s' \"$1\"\n",
    )
    .expect("script");
    let registry = Arc::new(SkillRegistry::discover(std::slice::from_ref(&root), 8));
    let tool = SkillScriptTool::new(registry, SkillScriptConfig::in_workspace(root.clone()));
    let output = tool
        .call(SkillScriptArgs {
            skill: "runner".to_owned(),
            script: "scripts/echo.sh".to_owned(),
            args: vec!["hello world".to_owned()],
            timeout_ms: Some(5_000),
        })
        .await
        .expect("script output");
    assert!(!output.is_error, "{output:?}");
    let text = format!("{:?}", output.parts);
    assert!(text.contains("hello world"));
    let escaped = tool
        .call(SkillScriptArgs {
            skill: "runner".to_owned(),
            script: "../echo.sh".to_owned(),
            args: Vec::new(),
            timeout_ms: None,
        })
        .await
        .expect("escape output");
    assert!(escaped.is_error);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn script_capture_keeps_at_most_the_limit() {
    let input = [b'x'; 32];
    let (bytes, truncated) = tools::read_limited(&input[..], 10).await.expect("read");
    assert_eq!(bytes.len(), 10);
    assert!(truncated);

    let (bytes, truncated) = tools::read_limited(&input[..], 64).await.expect("read");
    assert_eq!(bytes.len(), 32);
    assert!(!truncated);
}

#[test]
fn a_resource_read_is_bounded() {
    let root = temp_root("resource-bound");
    let directory = write_skill(
        &root,
        "bounded",
        "name: bounded\ndescription: Has a large reference.",
        "body",
    );
    std::fs::create_dir_all(directory.join("references")).expect("references");
    std::fs::write(directory.join("references/big.txt"), "x".repeat(1024)).expect("resource");
    let registry = SkillRegistry::discover(std::slice::from_ref(&root), 8);
    let skill = registry.get("bounded").expect("skill");
    let text = skill
        .read_resource("references/big.txt", 16)
        .expect("bounded read");
    assert!(text.starts_with(&"x".repeat(16)));
    assert!(text.contains("truncated at 16 bytes"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_huge_shebang_is_refused() {
    let root = temp_root("shebang");
    let script = root.join("huge.sh");
    let mut body = String::from("#!");
    body.push_str(&"x".repeat(5_000));
    body.push('\n');
    std::fs::write(&script, body).expect("script");
    let error = tools::interpreter(&script).expect_err("shebang must be bounded");
    assert!(error.contains("longer than"));
    let _ = std::fs::remove_dir_all(root);
}
