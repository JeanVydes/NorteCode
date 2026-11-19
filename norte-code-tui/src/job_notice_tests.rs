//! Text and tone of the background job notices.

use norte_code_wire::{JobInfo, JobState};

use super::{finished, started, tone};
use crate::ui::Tone;

fn job(state: JobState, exit_code: Option<i32>) -> JobInfo {
    JobInfo {
        id: "job-3".to_string(),
        label: "sleep 5".to_string(),
        script: String::new(),
        state,
        exit_code,
        timed_out: false,
        elapsed_ms: 5012,
        output: String::new(),
        dropped: false,
        output_bytes_total: 0,
    }
}

#[test]
fn a_started_job_names_the_id_and_command() {
    assert_eq!(
        started("job-3", "sleep 5"),
        "job job-3 started in background · sleep 5"
    );
}

#[test]
fn a_successful_job_reports_its_label_exit_and_time() {
    let notice = finished(&job(JobState::Finished, Some(0)));
    assert_eq!(notice, "job finished · sleep 5 · exit 0 · 5012 ms");
    assert_eq!(tone(&job(JobState::Finished, Some(0))), Tone::Neutral);
}

#[test]
fn a_failed_job_is_loud() {
    let notice = finished(&job(JobState::Finished, Some(1)));
    assert!(notice.contains("exit 1"));
    assert_eq!(tone(&job(JobState::Finished, Some(1))), Tone::Error);
}

#[test]
fn a_stopped_job_uses_the_id_when_it_has_no_label() {
    let mut stopped = job(JobState::Cancelled, None);
    stopped.label.clear();
    assert_eq!(finished(&stopped), "job stopped · job-3 · 5012 ms");
    assert_eq!(tone(&stopped), Tone::Warn);
}
