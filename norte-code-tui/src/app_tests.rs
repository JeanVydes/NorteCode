//! App-level clipboard routing and upload event folding.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use norte_code_wire::{SessionKey, UploadReceipt};
use terminalui::{InputEvent, Theme, Tree, node};
use tokio::sync::mpsc;

use super::{
    drain_plan_requests, drain_sessions_actions, fold, fold_upload_event, handle_key, sync_chrome,
};
use crate::attachment::{AttachmentKind, AttachmentSource, PreparedAttachment};
use crate::client::{ClientEvent, UiCommand};
use crate::clipboard::{Clipboard, FakeClipboard};
use crate::ui;
use crate::ui::Composer;
use crate::ui_model::{ActiveTurn, TurnState, UiModel};

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
fn ctrl_c_without_a_selection_quits() {
    let mut tree = composer_tree(Arc::new(FakeClipboard::default()));
    let mut model = UiModel::detached();
    assert!(handle_key(
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
        .open("provider", Vec::new());
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
fn ctrl_c_quits_without_cancelling_the_turn() {
    let mut tree = composer_tree(Arc::new(FakeClipboard::default()));
    let (commands_tx, mut commands_rx) = mpsc::channel(4);
    let mut commands = Some(commands_tx);
    let mut model = model_with_active_turn();

    assert!(handle_key(
        &mut tree,
        ctrl_c(),
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
        reason: "host refused the bytes".to_owned(),
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
