use norte_code_wire::{JobInfo, JobState};

use super::JobsPanel;

fn job(id: &str, label: &str, state: JobState, output: &str) -> JobInfo {
    JobInfo {
        id: id.to_owned(),
        label: label.to_owned(),
        script: label.to_owned(),
        state,
        exit_code: None,
        timed_out: false,
        elapsed_ms: 0,
        output: output.to_owned(),
        dropped: false,
        output_bytes_total: 0,
    }
}

#[test]
fn the_panel_counts_the_jobs_that_run() {
    let jobs = vec![
        job("job-1", "cargo build", JobState::Running, ""),
        job("job-2", "cargo test", JobState::Finished, ""),
    ];

    assert_eq!(JobsPanel::title(&jobs), "jobs 1/2");
}

#[test]
fn the_panel_shows_the_running_jobs_with_their_last_output() {
    let jobs = vec![job(
        "job-1",
        "cargo build",
        JobState::Running,
        "compiling\nlinking\n",
    )];

    let lines = JobsPanel::lines(&jobs);

    assert_eq!(lines.len(), 1);
    let text = lines[0]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(text.contains("cargo build"));
    assert!(text.contains("linking"));
}

#[test]
fn the_panel_leaves_finished_jobs_out() {
    let jobs = vec![job("job-1", "cargo build", JobState::Finished, "")];

    assert_eq!(JobsPanel::lines(&jobs).len(), 0);
    assert_eq!(JobsPanel::height(&jobs), 0);
}

#[test]
fn the_panel_keeps_room_for_the_transcript() {
    let jobs = vec![job("job-1", "cargo build", JobState::Running, "")];

    assert_eq!(JobsPanel::height(&jobs), 2);
}

#[test]
fn the_picker_offers_the_running_jobs() {
    let jobs = vec![
        job("job-1", "cargo build", JobState::Running, ""),
        job("job-2", "cargo test", JobState::Finished, ""),
    ];

    let lines = JobsPanel::picker_lines(&jobs, crate::ui_model::ThemeMode::Dark);

    assert_eq!(lines.len(), 1);
    let text = lines[0]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(text.starts_with("1 "));
    assert!(text.contains("cargo build"));
}
