//! App-level clipboard routing and upload event folding.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use norte_code_wire::{PluginId, ServerEvent, ServerEventKind, SessionKey, UploadReceipt};
use terminalui::{InputEvent, Theme, Tree, node};
use tokio::sync::mpsc;

use super::{
    chrome::sync_chrome,
    drain::drain_sessions_actions,
    event_fold::{drain_server_events, fold, fold_upload_event},
    input::{drain_plan_requests, handle_key},
};
use crate::attachment::{AttachmentKind, AttachmentSource, PreparedAttachment};
use crate::client::{ClientEvent, UiCommand};
use crate::clipboard::{Clipboard, FakeClipboard};
use crate::ui;
use crate::ui::Composer;
use crate::ui_model::{ActiveTurn, Tab, TurnState, UiModel};

fn composer_tree(clipboard: Arc<dyn Clipboard>) -> Tree {
    let mut tree = Tree::new(node(
        "composer",
        Composer::new(&Theme::default(), clipboard),
    ));
    assert!(tree.focus("composer"));
    tree
}

fn ctrl_c() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
}

fn ctrl_q() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)
}

fn escape() -> KeyEvent {
    KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
}

fn model_with_active_turn() -> UiModel {
    let mut model = UiModel::detached();
    model.turn = TurnState::Thinking;
    model.active_turn = Some(ActiveTurn {
        id: "turn_1".to_owned(),
    });
    model.active = Some(SessionKey::new("session_1"));
    model
}

fn receipt() -> UploadReceipt {
    UploadReceipt {
        upload_id: "upl_1".to_owned(),
        mime_type: "image/png".to_owned(),
        bytes: 12,
        sha256: "digest".to_owned(),
        expires_at: 0,
    }
}

fn image_event() -> ClientEvent {
    ClientEvent::Uploaded {
        attachment: PreparedAttachment {
            kind: AttachmentKind::Image,
            mime: crate::attachment::MimeType::png(),
            source: AttachmentSource::Image(vec![1, 2, 3]),
            cursor: 0,
        },
        receipt: receipt(),
    }
}

#[test]
fn a_session_renamed_event_updates_the_list_and_attached_title() {
    let key = SessionKey::new("session_1");
    let mut model = UiModel::detached();
    model.active = Some(key.clone());
    model.sessions = vec![norte_code_wire::SessionSummary {
        session_id: key.to_string(),
        ..Default::default()
    }];
    let clipboard: Arc<dyn Clipboard> = Arc::new(FakeClipboard::default());
    let mut tree = composer_tree(clipboard);
    let mut commands = None;

    fold(
        &ServerEvent {
            request_id: 1,
            session: Some(key),
            kind: ServerEventKind::SessionRenamed {
                title: "Login redirect fix".to_owned(),
            },
        },
        &mut tree,
        &mut model,
        &mut commands,
    );

    assert_eq!(
        model.sessions[0].title.as_deref(),
        Some("Login redirect fix")
    );
    assert_eq!(model.attached_title.as_deref(), Some("Login redirect fix"));
}

#[test]
fn a_session_renamed_event_reaches_the_rail() {
    let key = SessionKey::new("session_1");
    let mut model = UiModel::detached();
    model.active = Some(key.clone());
    let mut tree = ui::build_tree(&model);
    let mut commands = None;

    fold(
        &ServerEvent {
            request_id: 1,
            session: Some(key),
            kind: ServerEventKind::SessionRenamed {
                title: "Login redirect fix".to_owned(),
            },
        },
        &mut tree,
        &mut model,
        &mut commands,
    );
    sync_chrome(&mut tree, &model);

    assert_eq!(
        tree.find::<ui::Rail>("rail")
            .expect("rail")
            .title
            .as_deref(),
        Some("Login redirect fix")
    );
}

#[test]
fn ctrl_c_copies_the_composer_selection_without_quitting() {
    let clipboard = Arc::new(FakeClipboard::default());
    let shared: Arc<dyn Clipboard> = clipboard.clone();
    let mut tree = composer_tree(shared);
    for ch in "hello world".chars() {
        tree.handle_event(&InputEvent::Key(KeyEvent::new(
            KeyCode::Char(ch),
            KeyModifiers::NONE,
        )));
    }
    for _ in 0..5 {
        tree.handle_event(&InputEvent::Key(KeyEvent::new(
            KeyCode::Left,
            KeyModifiers::SHIFT,
        )));
    }
    let mut model = UiModel::detached();
    let quit = handle_key(&mut tree, ctrl_c(), &mut None, &mut None, &mut model, None);
    assert!(!quit);
    assert_eq!(clipboard.written(), vec!["world"]);
}

#[test]
fn ctrl_c_without_a_selection_does_not_quit() {
    let mut tree = composer_tree(Arc::new(FakeClipboard::default()));
    let mut model = UiModel::detached();
    assert!(!handle_key(
        &mut tree,
        ctrl_c(),
        &mut None,
        &mut None,
        &mut model,
        None
    ));
}

#[test]
fn escape_cancels_the_active_turn_and_keeps_the_tui_open() {
    let mut tree = composer_tree(Arc::new(FakeClipboard::default()));
    let (commands_tx, mut commands_rx) = mpsc::channel(4);
    let mut commands = Some(commands_tx);
    let mut model = model_with_active_turn();

    assert!(!handle_key(
        &mut tree,
        escape(),
        &mut commands,
        &mut None,
        &mut model,
        None
    ));
    assert_eq!(model.turn, TurnState::Idle);
    assert!(model.active_turn.is_none());
    assert!(
        matches!(commands_rx.try_recv(), Ok(UiCommand::CancelTurn { turn_id, .. }) if turn_id == "turn_1")
    );
}

#[test]
fn escape_closing_an_overlay_does_not_cancel_the_turn() {
    let mut model = model_with_active_turn();
    let mut tree = ui::build_tree(&model);
    tree.find_mut::<ui::KeyPrompt>("key_prompt")
        .expect("key prompt")
        .open("provider", &[]);
    ui::sync_views(&mut tree, &model);
    let (commands_tx, mut commands_rx) = mpsc::channel(4);
    let mut commands = Some(commands_tx);

    assert!(!handle_key(
        &mut tree,
        escape(),
        &mut commands,
        &mut None,
        &mut model,
        None
    ));
    assert!(commands_rx.try_recv().is_err());
    assert_eq!(model.turn, TurnState::Thinking);
    assert!(model.active_turn.is_some());
    assert!(
        !tree
            .find::<ui::KeyPrompt>("key_prompt")
            .expect("key prompt")
            .is_open()
    );
}

#[test]
fn ctrl_q_quits_without_cancelling_the_turn() {
    let mut tree = composer_tree(Arc::new(FakeClipboard::default()));
    let (commands_tx, mut commands_rx) = mpsc::channel(4);
    let mut commands = Some(commands_tx);
    let mut model = model_with_active_turn();

    assert!(handle_key(
        &mut tree,
        ctrl_q(),
        &mut commands,
        &mut None,
        &mut model,
        None
    ));
    assert!(commands_rx.try_recv().is_err());
    assert_eq!(model.turn, TurnState::Thinking);
    assert!(model.active_turn.is_some());
}

#[test]
fn an_uploaded_image_gets_its_chip_and_wire_part() {
    let mut tree = composer_tree(Arc::new(FakeClipboard::default()));
    assert!(fold_upload_event(&image_event(), &mut tree));
    let composer = tree.find::<Composer>("composer").expect("composer");
    assert_eq!(composer.expanded_text(), "[Image #1]");
    assert_eq!(composer.wire_parts().len(), 1);
}

#[test]
fn a_failed_upload_leaves_no_chip() {
    let mut tree = composer_tree(Arc::new(FakeClipboard::default()));
    let event = ClientEvent::UploadFailed {
        reason: crate::client_error::ClientError::upload("host refused the bytes"),
    };
    assert!(fold_upload_event(&event, &mut tree));
    let composer = tree.find::<Composer>("composer").expect("composer");
    assert_eq!(composer.expanded_text(), "");
    assert_eq!(composer.wire_parts(), []);
}

fn turn_finished() -> norte_code_wire::ServerEvent {
    norte_code_wire::ServerEvent {
        request_id: 1,
        session: Some(SessionKey::new("session_1")),
        kind: norte_code_wire::ServerEventKind::TurnFinished {
            turn: 1,
            phase: norte_code_wire::Phase::Idle,
            committed: true,
            output_schema_valid: None,
        },
    }
}

#[test]
fn turn_finish_refreshes_the_rail() {
    let mut tree = ui::build_tree(&UiModel::detached());
    let mut model = UiModel::detached();
    let (commands_tx, mut commands_rx) = mpsc::channel(4);
    let mut commands = Some(commands_tx);

    fold(&turn_finished(), &mut tree, &mut model, &mut commands);

    assert!(matches!(commands_rx.try_recv(), Ok(UiCommand::RefreshRail)));
}

#[test]
fn a_started_job_appears_in_the_rail_model() {
    let mut tree = ui::build_tree(&UiModel::detached());
    let mut model = UiModel::detached();
    let mut commands = None;
    let event = norte_code_wire::ServerEvent {
        request_id: 1,
        session: Some(SessionKey::new("session_1")),
        kind: norte_code_wire::ServerEventKind::JobStarted {
            job_id: "job_1".to_owned(),
            tool_call_id: "call_1".to_owned(),
            command: "echo hello".to_owned(),
            started_at: 0,
            limits: norte_code_wire::JobLimits::default(),
        },
    };

    fold(&event, &mut tree, &mut model, &mut commands);

    assert_eq!(model.jobs.len(), 1);
    assert_eq!(model.jobs[0].job_id, "job_1");
    assert_eq!(model.jobs[0].command, "echo hello");
}

#[test]
fn a_session_todos_event_updates_the_rail_model() {
    use norte_code_wire::{TodoInfo, TodoStatus};

    let mut tree = ui::build_tree(&UiModel::detached());
    let mut model = UiModel::detached();
    let mut commands = None;
    let event = norte_code_wire::ServerEvent {
        request_id: 1,
        session: Some(SessionKey::new("session_1")),
        kind: norte_code_wire::ServerEventKind::SessionTodos {
            todos: vec![TodoInfo {
                content: "write tests".to_owned(),
                status: TodoStatus::InProgress,
            }],
        },
    };

    fold(&event, &mut tree, &mut model, &mut commands);

    assert_eq!(model.todos.len(), 1);
    assert_eq!(model.todos[0].content, "write tests");
    assert_eq!(model.todos[0].status, TodoStatus::InProgress);
}

#[test]
fn a_plugin_message_event_is_drawn_in_the_transcript() {
    let mut tree = ui::build_tree(&UiModel::detached());
    let mut model = UiModel::detached();
    let mut commands = None;
    let event = norte_code_wire::ServerEvent {
        request_id: 1,
        session: Some(SessionKey::new("session_1")),
        kind: ServerEventKind::PluginMessageInjected {
            plugin_id: PluginId::new("browser"),
            call_id: "call_1".to_owned(),
            parts: 2,
        },
    };

    fold(&event, &mut tree, &mut model, &mut commands);

    let transcript = tree.find::<ui::Transcript>("stream").expect("stream");
    let snapshot = serde_json::to_string(transcript).expect("transcript snapshot");
    assert!(snapshot.contains("plugin browser added a message with 2 part(s)"));
}

#[test]
fn a_plugin_message_rejection_is_drawn_as_a_warning() {
    let mut tree = ui::build_tree(&UiModel::detached());
    let mut model = UiModel::detached();
    let mut commands = None;
    let event = norte_code_wire::ServerEvent {
        request_id: 1,
        session: Some(SessionKey::new("session_1")),
        kind: ServerEventKind::PluginMessageRejected {
            plugin_id: PluginId::new("browser"),
            call_id: "call_1".to_owned(),
            reason: norte_code_wire::PluginMessageRejection::MediaNotAllowed,
        },
    };

    fold(&event, &mut tree, &mut model, &mut commands);

    let transcript = tree.find::<ui::Transcript>("stream").expect("stream");
    let snapshot = serde_json::to_string(transcript).expect("transcript snapshot");
    assert!(snapshot.contains("media is not allowed in plugin messages"));
    assert!(snapshot.contains("warn"));
}

#[test]
fn the_new_session_button_sends_the_command() {
    let mut model = UiModel::detached();
    model.tab = crate::ui_model::Tab::Sessions;
    let mut tree = ui::build_tree(&model);
    ui::sync_views(&mut tree, &model);
    tree.find_mut::<ui::SessionsPage>("sessions")
        .expect("sessions")
        .set_data(Vec::new(), None, Arc::default());
    let theme = model.preferences.catalog.theme(&model.preferences.theme);
    let area = ratatui::layout::Rect::new(0, 0, 100, 24);
    let mut buf = ratatui::buffer::Buffer::empty(area);
    tree.render(area, &mut buf, &theme);

    press_button(&mut tree, "new_session");

    let (commands_tx, mut commands_rx) = mpsc::channel(4);
    let mut commands = Some(commands_tx);
    drain_sessions_actions(&mut tree, &mut commands, &mut model);
    assert!(matches!(
        commands_rx.try_recv(),
        Ok(UiCommand::CreateSession { title: None })
    ));
}

#[test]
fn creating_a_session_switches_to_chat_and_sends_the_command() {
    let mut model = UiModel::detached();
    model.tab = crate::ui_model::Tab::Sessions;
    let mut tree = ui::build_tree(&model);
    let (commands_tx, mut commands_rx) = mpsc::channel(4);
    let mut commands = Some(commands_tx);
    tree.find_mut::<ui::SessionsPage>("sessions")
        .expect("sessions")
        .set_data(Vec::new(), None, Arc::default());
    ui::sync_views(&mut tree, &model);
    let new_key = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE);
    let _ = tree.handle_event(&InputEvent::Key(new_key));

    drain_sessions_actions(&mut tree, &mut commands, &mut model);

    assert!(matches!(
        commands_rx.try_recv(),
        Ok(UiCommand::CreateSession { title: None })
    ));
    assert_eq!(model.tab, crate::ui_model::Tab::Chat);
}

fn plan(id: &str) -> norte_code_wire::PlanSummary {
    norte_code_wire::PlanSummary {
        id: id.to_owned(),
        title: Some(format!("Plan {id}")),
        content: "body".to_owned(),
        ..Default::default()
    }
}

fn press_button(tree: &mut Tree, id: &str) {
    let button = tree
        .find::<terminalui::components::button::Button>(id)
        .unwrap_or_else(|| panic!("button {id}"));
    let click = InputEvent::Mouse(crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: button.state.area.x,
        row: button.state.area.y,
        modifiers: crossterm::event::KeyModifiers::NONE,
    });
    let _ = tree.handle_event(&click);
}

fn rendered_tree(model: &UiModel, height: u16) -> Tree {
    let mut tree = ui::build_tree(model);
    sync_chrome(&mut tree, model);
    let theme = model.preferences.catalog.theme(&model.preferences.theme);
    let area = ratatui::layout::Rect::new(0, 0, 100, height);
    let mut buf = ratatui::buffer::Buffer::empty(area);
    tree.render(area, &mut buf, &theme);
    tree
}

#[test]
fn a_refused_prompt_clears_the_loading_row() {
    let mut model = UiModel::detached();
    let mut tree = ui::build_tree(&model);
    tree.find_mut::<ui::Transcript>("stream")
        .expect("stream")
        .start_thinking();
    assert!(painted_text(&mut tree, &model).contains("thinking"));

    let (send, receive) = mpsc::channel(4);
    send.try_send(ClientEvent::SendFailed {
        reason: crate::client_error::ClientError::send_prompt("session busy"),
    })
    .expect("queued failure");
    let mut events = Some(receive);
    let mut commands = None;

    let outcome = drain_server_events(&mut events, &mut commands, &mut tree, &mut model);

    assert_eq!(outcome.events, 1);
    assert!(!painted_text(&mut tree, &model).contains("thinking"));
}

fn painted_text(tree: &mut Tree, model: &UiModel) -> String {
    let theme = model.preferences.catalog.theme(&model.preferences.theme);
    let area = ratatui::layout::Rect::new(0, 0, 100, 24);
    let mut buf = ratatui::buffer::Buffer::empty(area);
    tree.render(area, &mut buf, &theme);
    let sheet = &buf;
    (0..area.height)
        .flat_map(|y| (0..area.width).map(move |x| sheet[(x, y)].symbol().to_string()))
        .collect()
}

#[test]
fn closing_rename_returns_focus_to_the_sessions_page() {
    let mut model = UiModel::detached();
    model.tab = Tab::Sessions;
    let mut tree = ui::build_tree(&model);
    tree.find_mut::<ui::SessionsPage>("sessions")
        .expect("sessions page")
        .set_data(
            vec![norte_code_wire::SessionSummary {
                session_id: "session_1".to_owned(),
                title: Some("original name".to_owned()),
                ..Default::default()
            }],
            None,
            crate::session_stats::SharedUsage::default(),
        );
    ui::sync_views(&mut tree, &model);
    assert!(tree.focus("sessions"));
    let mut commands = None;
    let mut events = None;

    let rename = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE);
    handle_key(
        &mut tree,
        rename,
        &mut commands,
        &mut events,
        &mut model,
        None,
    );
    assert_eq!(tree.focused_id(), Some("title:input"));

    let confirm = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    handle_key(
        &mut tree,
        confirm,
        &mut commands,
        &mut events,
        &mut model,
        None,
    );
    assert_eq!(tree.focused_id(), Some("sessions"));
}

#[test]
fn background_jobs_are_not_shown_in_the_rail() {
    let mut model = UiModel::detached();
    model.jobs = vec![norte_code_wire::JobSummary {
        job_id: "job_1".to_owned(),
        command: "echo hello".to_owned(),
        ..Default::default()
    }];
    let mut tree = ui::build_tree(&model);
    sync_chrome(&mut tree, &model);
    let theme = model.preferences.catalog.theme(&model.preferences.theme);
    let area = ratatui::layout::Rect::new(0, 0, 100, 40);
    let mut buf = ratatui::buffer::Buffer::empty(area);

    tree.render(area, &mut buf, &theme);

    let sheet = &buf;
    let painted: String = (0..area.height)
        .flat_map(|y| (0..28).map(move |x| sheet[(x, y)].symbol().to_string()))
        .collect();
    assert!(!painted.contains("JOBS"), "{painted}");
    assert!(!painted.contains("echo hello"), "{painted}");
}

#[test]
fn each_preview_button_opens_its_own_plan() {
    let mut model = UiModel::detached();
    for index in 0..4 {
        model.plans.push(plan(&format!("plan_{index}")));
    }
    let mut tree = rendered_tree(&model, 24);

    press_button(&mut tree, "preview_3");
    drain_plan_requests(&mut tree, &model);

    let overlay = tree
        .find::<ui::PlanOverlay>("plan_overlay")
        .expect("plan overlay");
    assert_eq!(overlay.plan().map(|plan| plan.id.as_str()), Some("plan_3"));
}

fn tool_finished(tool: &str, is_error: bool) -> norte_code_wire::ServerEvent {
    norte_code_wire::ServerEvent {
        request_id: 1,
        session: Some(SessionKey::new("session_1")),
        kind: ServerEventKind::ToolFinished {
            turn: 1,
            call_id: "call_1".to_owned(),
            tool: tool.to_owned(),
            is_error,
            error: None,
            output: None,
            truncated: false,
            detached: false,
            duration_ms: 1,
        },
    }
}

fn plan_review_asked() -> norte_code_wire::ServerEvent {
    norte_code_wire::ServerEvent {
        request_id: 1,
        session: Some(SessionKey::new("session_1")),
        kind: ServerEventKind::QuestionAsked {
            turn: 1,
            question: "q_1".to_owned(),
            prompt: format!("{}# Plan\n\nDo the work", ui::PLAN_REVIEW_PREFIX),
            options: vec!["Approve".to_owned(), "Reject".to_owned()],
            multiple: false,
        },
    }
}

#[test]
fn escape_closing_the_plan_preview_does_not_cancel_the_turn() {
    let mut model = model_with_active_turn();
    for index in 0..4 {
        model.plans.push(plan(&format!("plan_{index}")));
    }
    let mut tree = rendered_tree(&model, 24);
    press_button(&mut tree, "preview_3");
    drain_plan_requests(&mut tree, &model);
    ui::sync_views(&mut tree, &model);
    assert!(
        tree.find::<ui::PlanOverlay>("plan_overlay")
            .expect("plan overlay")
            .is_open()
    );
    let (commands_tx, mut commands_rx) = mpsc::channel(4);
    let mut commands = Some(commands_tx);

    assert!(!handle_key(
        &mut tree,
        escape(),
        &mut commands,
        &mut None,
        &mut model,
        None
    ));
    let queued = commands_rx.try_recv();
    assert!(queued.is_err(), "queued command: {queued:?}");
    assert_eq!(model.turn, TurnState::Thinking);
    assert!(model.active_turn.is_some());
    assert!(
        !tree
            .find::<ui::PlanOverlay>("plan_overlay")
            .expect("plan overlay")
            .is_open()
    );
}

#[test]
fn a_finished_plan_tool_refreshes_the_rail_immediately() {
    let mut tree = ui::build_tree(&UiModel::detached());
    let mut model = UiModel::detached();
    let (commands_tx, mut commands_rx) = mpsc::channel(4);
    let mut commands = Some(commands_tx);

    fold(
        &tool_finished("plan", false),
        &mut tree,
        &mut model,
        &mut commands,
    );

    assert!(matches!(commands_rx.try_recv(), Ok(UiCommand::RefreshRail)));
}

#[test]
fn a_failed_plan_tool_does_not_refresh_the_rail() {
    let mut tree = ui::build_tree(&UiModel::detached());
    let mut model = UiModel::detached();
    let (commands_tx, mut commands_rx) = mpsc::channel(4);
    let mut commands = Some(commands_tx);

    fold(
        &tool_finished("plan", true),
        &mut tree,
        &mut model,
        &mut commands,
    );

    assert!(commands_rx.try_recv().is_err());
}

#[test]
fn an_unrelated_finished_tool_does_not_refresh_the_rail() {
    let mut tree = ui::build_tree(&UiModel::detached());
    let mut model = UiModel::detached();
    let (commands_tx, mut commands_rx) = mpsc::channel(4);
    let mut commands = Some(commands_tx);

    fold(
        &tool_finished("shell", false),
        &mut tree,
        &mut model,
        &mut commands,
    );

    assert!(commands_rx.try_recv().is_err());
}

#[test]
fn accepting_a_plan_review_answers_the_pending_question() {
    let mut tree = ui::build_tree(&UiModel::detached());
    let mut model = model_with_active_turn();
    fold(&plan_review_asked(), &mut tree, &mut model, &mut None);
    ui::sync_views(&mut tree, &model);
    let (commands_tx, mut commands_rx) = mpsc::channel(4);
    let mut commands = Some(commands_tx);

    handle_key(
        &mut tree,
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
        &mut commands,
        &mut None,
        &mut model,
        None,
    );

    match commands_rx.try_recv() {
        Ok(UiCommand::AnswerQuestion {
            question, answer, ..
        }) => {
            assert_eq!(question, "q_1");
            assert_eq!(answer, crate::answer::QuestionAnswer::approve());
        }
        other => panic!("queued command: {other:?}"),
    }
    assert!(model.question.is_none());
    assert!(
        !tree
            .find::<ui::PlanOverlay>("plan_overlay")
            .expect("plan overlay")
            .is_open()
    );
}

#[test]
fn feedback_on_a_plan_review_sends_the_written_text() {
    let mut tree = ui::build_tree(&UiModel::detached());
    let mut model = model_with_active_turn();
    fold(&plan_review_asked(), &mut tree, &mut model, &mut None);
    ui::sync_views(&mut tree, &model);
    let (commands_tx, mut commands_rx) = mpsc::channel(4);
    let mut commands = Some(commands_tx);

    for key in [
        KeyCode::Char('f'),
        KeyCode::Char('n'),
        KeyCode::Char('o'),
        KeyCode::Enter,
    ] {
        handle_key(
            &mut tree,
            KeyEvent::new(key, KeyModifiers::NONE),
            &mut commands,
            &mut None,
            &mut model,
            None,
        );
    }

    match commands_rx.try_recv() {
        Ok(UiCommand::AnswerQuestion {
            question, answer, ..
        }) => {
            assert_eq!(question, "q_1");
            assert_eq!(
                answer,
                crate::answer::QuestionAnswer::new("no").expect("a typed answer")
            );
        }
        other => panic!("queued command: {other:?}"),
    }
    assert!(model.question.is_none());
}

#[test]
fn ctrl_d_opens_and_enter_confirms_the_delete_dialog() {
    use std::collections::HashMap;

    let mut model = UiModel::detached();
    model.tab = Tab::Models;
    let mut tree = ui::build_tree(&model);
    ui::sync_views(&mut tree, &model);
    let page = tree.find_mut::<ui::ModelsPage>("models").expect("models");
    page.set_data(ui::ModelsData {
        models: vec![crate::test_support::model_info("openrouter", "one")],
        key_status: HashMap::from([(
            "openrouter".to_owned(),
            crate::test_support::provider_key_status(norte_code_wire::ProviderKeySource::File, &[]),
        )]),
        active: None,
    });
    assert!(tree.focus("models"));

    let (send, mut commands_rx) = mpsc::channel(4);
    let mut commands = Some(send);
    let mut events: Option<mpsc::Receiver<ClientEvent>> = None;
    let ctrl_d = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);
    handle_key(
        &mut tree,
        ctrl_d,
        &mut commands,
        &mut events,
        &mut model,
        None,
    );
    assert_eq!(
        tree.focused_id(),
        Some("delete_key_confirm"),
        "{}",
        tree.inspect_outline()
    );

    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    handle_key(
        &mut tree,
        enter,
        &mut commands,
        &mut events,
        &mut model,
        None,
    );
    match commands_rx.try_recv() {
        Ok(UiCommand::DeleteSecret { provider }) => assert_eq!(provider, "openrouter"),
        other => panic!("unexpected command: {other:?}"),
    }
    assert!(
        !tree
            .find::<ui::ModelsPage>("models")
            .expect("models")
            .confirming_delete()
    );
}

#[test]
fn tab_and_arrows_navigate_the_delete_dialog_before_enter() {
    use std::collections::HashMap;

    let mut model = UiModel::detached();
    model.tab = Tab::Models;
    let mut tree = ui::build_tree(&model);
    ui::sync_views(&mut tree, &model);
    tree.find_mut::<ui::ModelsPage>("models")
        .expect("models")
        .set_data(ui::ModelsData {
            models: vec![crate::test_support::model_info("openrouter", "one")],
            key_status: HashMap::from([(
                "openrouter".to_owned(),
                crate::test_support::provider_key_status(
                    norte_code_wire::ProviderKeySource::File,
                    &[],
                ),
            )]),
            active: None,
        });

    let (send, mut commands_rx) = mpsc::channel(4);
    let mut commands = Some(send);
    let mut events: Option<mpsc::Receiver<ClientEvent>> = None;
    let mut press = |tree: &mut Tree, key: KeyEvent| {
        handle_key(tree, key, &mut commands, &mut events, &mut model, None);
    };

    press(
        &mut tree,
        KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
    );
    assert_eq!(
        tree.find::<ui::ModelsPage>("models")
            .expect("models")
            .focus_id(),
        Some("delete_key_confirm")
    );

    press(&mut tree, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(
        tree.find::<ui::ModelsPage>("models")
            .expect("models")
            .focus_id(),
        Some("delete_key_cancel")
    );
    press(&mut tree, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert_eq!(
        tree.find::<ui::ModelsPage>("models")
            .expect("models")
            .focus_id(),
        Some("delete_key_confirm")
    );
    press(&mut tree, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    press(&mut tree, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(commands_rx.try_recv().is_err());
    assert!(
        !tree
            .find::<ui::ModelsPage>("models")
            .expect("models")
            .confirming_delete()
    );
}

#[test]
fn deleting_an_environment_key_shows_a_toast() {
    use std::collections::HashMap;

    let mut model = UiModel::detached();
    model.tab = Tab::Models;
    let mut tree = ui::build_tree(&model);
    ui::sync_views(&mut tree, &model);
    tree.find_mut::<ui::ModelsPage>("models")
        .expect("models")
        .set_data(ui::ModelsData {
            models: vec![crate::test_support::model_info("openrouter", "one")],
            key_status: HashMap::from([(
                "openrouter".to_owned(),
                crate::test_support::provider_key_status(
                    norte_code_wire::ProviderKeySource::Environment,
                    &["OPENROUTER_API_KEY"],
                ),
            )]),
            active: None,
        });
    assert!(tree.focus("models"));

    let (send, mut commands_rx) = mpsc::channel(4);
    let mut commands = Some(send);
    let mut events: Option<mpsc::Receiver<ClientEvent>> = None;
    handle_key(
        &mut tree,
        KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
        &mut commands,
        &mut events,
        &mut model,
        None,
    );

    assert!(
        tree.find::<ui::ToastHost>("toast")
            .expect("toast")
            .is_active()
    );
    let painted = crate::test_support::rendered(&mut tree);
    assert!(painted.contains("only saved keys can be removed"));
    assert!(
        !tree
            .find::<ui::ModelsPage>("models")
            .expect("models")
            .confirming_delete()
    );
    assert!(commands_rx.try_recv().is_err());
}

#[test]
fn enter_confirms_delete_and_closes_the_overlay() {
    let mut model = UiModel::detached();
    model.tab = Tab::Sessions;
    let mut tree = ui::build_tree(&model);
    tree.find_mut::<ui::SessionsPage>("sessions")
        .expect("sessions page")
        .set_data(
            vec![norte_code_wire::SessionSummary {
                session_id: "doomed".to_owned(),
                ..Default::default()
            }],
            None,
            crate::session_stats::SharedUsage::default(),
        );
    ui::sync_views(&mut tree, &model);
    assert!(tree.focus("sessions"));
    let mut commands = None;
    let mut events = None;

    handle_key(
        &mut tree,
        KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        &mut commands,
        &mut events,
        &mut model,
        None,
    );
    assert_eq!(tree.focused_id(), Some("delete_confirm"));

    handle_key(
        &mut tree,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut commands,
        &mut events,
        &mut model,
        None,
    );

    assert_eq!(
        tree.focused_id(),
        Some("sessions"),
        "confirm must close the delete overlay and return focus"
    );
}
