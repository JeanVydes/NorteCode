use super::*;
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use norte::{FnExecutor, SessionId, ToolContext};

use super::web_fetch::{blocked_host, blocked_ip, parse_web_url};

fn temp_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("norte-code-tools-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).expect("src");
    std::fs::write(root.join("src/lib.rs"), "fn one() {}\nfn two() {}\n").expect("lib");
    std::fs::write(root.join("README.md"), "# readme\n").expect("readme");
    root
}

#[tokio::test]
async fn grep_reports_path_and_line() {
    let root = temp_root("grep");
    let tool = GrepTool::new(&root);
    let output = tool
        .call(GrepArgs {
            pattern: "fn ".to_owned(),
            path: None,
            glob: Some("**/*.rs".to_owned()),
            ignore_case: false,
            max_matches: Some(1),
            offset: Some(1),
        })
        .await
        .expect("grep");
    let text = format!("{:?}", output.parts);
    assert!(text.contains("src/lib.rs"));
    assert!(text.contains("fn two"));
    assert!(!text.contains("fn one"));
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn grep_skips_hidden_paths_and_honors_gitignore_without_git() {
    let root = temp_root("grep-ignore");
    std::fs::write(root.join(".gitignore"), "ignored.txt\n").expect("gitignore");
    std::fs::write(root.join("ignored.txt"), "needle\n").expect("ignored");
    std::fs::write(root.join("visible.txt"), "needle\n").expect("visible");
    std::fs::create_dir_all(root.join(".hidden")).expect("hidden dir");
    std::fs::write(root.join(".hidden/secret.txt"), "needle\n").expect("secret");
    assert!(!root.join(".git").exists());

    let tool = GrepTool::new(&root);
    let output = tool
        .call(GrepArgs {
            pattern: "needle".to_owned(),
            path: None,
            glob: Some("**/*.txt".to_owned()),
            ignore_case: false,
            max_matches: None,
            offset: None,
        })
        .await
        .expect("grep");
    let text = format!("{:?}", output.parts);
    assert!(text.contains("visible.txt"), "{text}");
    assert!(!text.contains("ignored.txt"), "{text}");
    assert!(!text.contains("secret.txt"), "{text}");
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn todo_replaces_the_list() {
    let state = Arc::new(TodoState::default());
    let tool = TodoTool::new(Arc::clone(&state));
    let output = tool
        .call(TodoArgs {
            todos: vec![
                TodoItem {
                    content: "read".to_owned(),
                    status: TodoStatus::Completed,
                },
                TodoItem {
                    content: "write".to_owned(),
                    status: TodoStatus::Pending,
                },
            ],
        })
        .await
        .expect("todo");
    let text = format!("{:?}", output.parts);
    assert!(text.contains("1/2 done"));
    assert_eq!(state.list(None).len(), 2);
}

#[tokio::test]
async fn a_question_is_answered_through_the_broker() {
    let broker = Arc::new(QuestionBroker::new());
    let mut events = broker.subscribe();
    let asking = Arc::clone(&broker);
    let task = tokio::spawn(async move {
        asking
            .ask(
                &ToolContext::new(),
                "Continue?".to_owned(),
                vec!["Yes".to_owned(), "No".to_owned()],
                false,
            )
            .await
    });
    let event = events.recv().await.expect("question event");
    assert_eq!(event.question, "Continue?");
    assert_eq!(event.options, ["Yes", "No"]);
    assert!(!event.multiple);
    assert!(broker.answer(None, &event.id, "yes".to_owned()).await);
    let answer = task.await.expect("task").expect("answer");
    assert_eq!(answer, "yes");
}

#[derive(Default)]
struct MemoryQuestionStore {
    questions: Mutex<BTreeMap<String, DurableQuestion>>,
}

#[async_trait]
impl QuestionStore for MemoryQuestionStore {
    async fn load(&self, call_id: &str) -> Option<DurableQuestion> {
        self.questions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(call_id)
            .cloned()
    }

    async fn save(&self, question: &DurableQuestion) {
        self.questions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(question.call_id.clone(), question.clone());
    }

    async fn answer(&self, call_id: &str, answer: &str) -> bool {
        let mut questions = self
            .questions
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(question) = questions.get_mut(call_id) else {
            return false;
        };
        if question.answer.is_some() {
            return false;
        }
        question.answer = Some(answer.to_owned());
        true
    }
}

#[tokio::test]
async fn a_stored_question_is_reasked_then_replayed_across_brokers() {
    let store = Arc::new(MemoryQuestionStore::default());
    let context = ToolContext::new()
        .with_session(SessionId::new())
        .with_call(norte::CallId::new());

    let first = Arc::new(QuestionBroker::new().with_store(Arc::clone(&store) as _));
    let mut events = first.subscribe();
    let asking = Arc::clone(&first);
    let task = tokio::spawn({
        let context = context.clone();
        async move {
            asking
                .ask(
                    &context,
                    "Continue?".to_owned(),
                    vec!["Yes".to_owned(), "No".to_owned()],
                    true,
                )
                .await
        }
    });
    let asked = events.recv().await.expect("question event");
    task.abort();

    let second = Arc::new(QuestionBroker::new().with_store(Arc::clone(&store) as _));
    let mut events = second.subscribe();
    let asking = Arc::clone(&second);
    let task = tokio::spawn({
        let context = context.clone();
        async move {
            asking
                .ask(
                    &context,
                    "Continue?".to_owned(),
                    vec!["Yes".to_owned(), "No".to_owned()],
                    true,
                )
                .await
        }
    });
    let reasked = events.recv().await.expect("question event");
    assert_eq!(reasked.id, asked.id);
    assert_eq!(reasked.options, ["Yes", "No"]);
    assert!(reasked.multiple);
    assert!(
        second
            .answer(context.session, &reasked.id, "yes".to_owned())
            .await
    );
    assert_eq!(task.await.expect("task").expect("answer"), "yes");

    let third = Arc::new(QuestionBroker::new().with_store(store as _));
    let answer = tokio::time::timeout(
        Duration::from_secs(1),
        third.ask(&context, "Continue?".to_owned(), Vec::new(), false),
    )
    .await
    .expect("a replayed answer returns immediately")
    .expect("answer");
    assert_eq!(answer, "yes");
}

#[test]
fn escape_hides_bidi_overrides() {
    let text = "safe\u{202e}txet_dexe\u{2066}safe\u{061c}\u{200b}hidden\u{feff}bom";
    let escaped = escape(text);
    assert!(!escaped.chars().any(is_bidi));
    assert!(escaped.contains("\\u{202e}"));
    assert!(escaped.contains("\\u{2066}"));
    assert!(escaped.contains("\\u{61c}"));
    assert!(escaped.contains("\\u{200b}"));
    assert!(escaped.contains("\\u{feff}"));
}

#[test]
fn web_fetch_blocks_non_public_addresses() {
    for address in [
        "0.0.0.0",
        "127.0.0.1",
        "10.0.0.1",
        "172.16.0.1",
        "192.168.1.1",
        "169.254.169.254",
        "100.100.100.200",
        "192.0.0.1",
        "192.0.2.1",
        "198.18.0.1",
        "224.0.0.1",
        "240.0.0.1",
        "255.255.255.255",
        "::",
        "::1",
        "::ffff:127.0.0.1",
        "fd00:ec2::254",
        "fe80::1",
        "2001:db8::1",
    ] {
        let ip = address.parse().expect("test address");
        assert!(blocked_ip(ip), "{address} must be blocked");
    }
    for address in ["1.1.1.1", "8.8.8.8", "2606:4700:4700::1111"] {
        let ip = address.parse().expect("test address");
        assert!(!blocked_ip(ip), "{address} must be allowed");
    }
}

#[test]
fn web_fetch_blocks_local_and_metadata_hosts() {
    for host in [
        "localhost",
        "LOCALHOST.",
        "api.localhost",
        "metadata.google.internal",
        "printer.internal",
    ] {
        assert!(blocked_host(host), "{host} must be blocked");
    }
    for host in [
        "example.com",
        "notlocalhost",
        "internal.example.com",
        "localhost.example.com",
    ] {
        assert!(!blocked_host(host), "{host} must be allowed");
    }
}

#[test]
fn web_fetch_accepts_only_http_without_credentials() {
    assert!(parse_web_url("https://example.com/a").is_ok());
    assert!(parse_web_url("http://example.com:8080/a").is_ok());
    for url in [
        "ftp://example.com/a",
        "file:///etc/passwd",
        "https://user:pass@example.com/",
    ] {
        assert!(parse_web_url(url).is_err(), "{url} must be rejected");
    }
}

#[tokio::test]
async fn web_fetch_refuses_local_targets_before_connecting() {
    let tool = WebFetchTool::new();
    for url in [
        "http://127.0.0.1/",
        "http://[::1]/",
        "http://10.0.0.1/",
        "http://169.254.169.254/latest/meta-data/",
        "http://localhost/",
        "http://metadata.google.internal/",
    ] {
        let error = tool.fetch(url).await.expect_err("target must be refused");
        assert!(
            error.contains("private") || error.contains("public host"),
            "{url}: {error}"
        );
    }
}

#[test]
fn clip_line_counts_characters_not_bytes() {
    let exact = "ñ".repeat(MAX_LINE_WIDTH);
    assert_eq!(clip_line(&exact), exact);

    let long = "ñ".repeat(MAX_LINE_WIDTH + 1);
    let clipped = clip_line(&long);
    assert!(clipped.ends_with('…'));
    assert_eq!(clipped.chars().count(), MAX_LINE_WIDTH + 1);
}
