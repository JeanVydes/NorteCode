use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use norte_code_wire::{JobInfo, JobState};

use super::JobsView;
use crate::components::{Component, ComponentAction, ViewAction};

fn job(id: &str, label: &str, state: JobState, output: &str) -> JobInfo {
    JobInfo {
        id: id.to_owned(),
        label: label.to_owned(),
        script: label.to_owned(),
        state,
        exit_code: match state {
            JobState::Finished => Some(0),
            _ => None,
        },
        timed_out: false,
        elapsed_ms: 1_500,
        output: output.to_owned(),
        dropped: false,
        output_bytes_total: 0,
    }
}

fn model(with: Vec<JobInfo>) -> crate::ui_model::UiModel {
    crate::ui_model::UiModel {
        jobs: with,
        ..crate::ui_model::UiModel::default()
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn the_refresh_key_asks_the_host_again() {
    let mut view = JobsView::default();
    let model = model(vec![]);

    view.handle_key(key(KeyCode::Char('r')), &model);

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::View(ViewAction::RefreshJobs))
    ));
}

#[test]
fn the_stop_key_stops_the_job_the_reader_selected() {
    let mut view = JobsView::default();
    let model = model(vec![
        job("job-1", "cargo build", JobState::Running, ""),
        job("job-2", "cargo test", JobState::Running, ""),
    ]);

    view.handle_key(key(KeyCode::Down), &model);
    view.handle_key(key(KeyCode::Char('S')), &model);

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::StopJob { job_id }) if job_id == "job-2"
    ));
}

#[test]
fn the_stop_key_leaves_finished_jobs_alone() {
    let mut view = JobsView::default();
    let model = model(vec![job("job-1", "cargo build", JobState::Finished, "")]);

    view.handle_key(key(KeyCode::Char('S')), &model);

    assert!(view.take_action().is_none());
}

#[test]
fn the_arrow_keys_move_the_selection_within_the_list() {
    let mut view = JobsView::default();
    let model = model(vec![
        job("job-1", "cargo build", JobState::Running, ""),
        job("job-2", "cargo test", JobState::Running, ""),
    ]);

    // Up at the top stays put; down past the end stays put.
    view.handle_key(key(KeyCode::Up), &model);
    view.handle_key(key(KeyCode::Down), &model);
    view.handle_key(key(KeyCode::Down), &model);
    view.handle_key(key(KeyCode::Down), &model);
    view.handle_key(key(KeyCode::Char('S')), &model);

    assert!(matches!(
        view.take_action(),
        Some(ComponentAction::StopJob { job_id }) if job_id == "job-2"
    ));
}

#[test]
fn the_page_draws_every_job_with_its_state() {
    let mut view = JobsView::default();
    let model = model(vec![
        job(
            "job-1",
            "cargo build",
            JobState::Running,
            "compiling\nlinking\n",
        ),
        job(
            "job-2",
            "cargo test",
            JobState::Finished,
            "test result: ok\n",
        ),
    ]);
    let backend = ratatui::backend::TestBackend::new(80, 10);
    let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| view.render(frame, frame.area(), &model))
        .expect("render");
    let drawn = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect::<String>();

    for expected in ["cargo build", "running", "linking", "cargo test", "exit 0"] {
        assert!(drawn.contains(expected), "{expected} is drawn:\n{drawn}");
    }
}

#[test]
fn the_page_says_when_there_is_nothing_to_list() {
    let mut view = JobsView::default();
    let model = model(vec![]);
    let backend = ratatui::backend::TestBackend::new(80, 6);
    let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| view.render(frame, frame.area(), &model))
        .expect("render");
    let drawn = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect::<String>();

    assert!(drawn.contains("No jobs in this session"));
}

#[test]
fn the_elapsed_label_reads_the_way_the_footer_does() {
    assert_eq!(JobsView::elapsed(500), "500ms");
    assert_eq!(JobsView::elapsed(1_500), "1.5s");
    assert_eq!(JobsView::elapsed(90_000), "1m30s");
}
