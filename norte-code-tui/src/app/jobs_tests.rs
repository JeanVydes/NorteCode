use norte_code_wire::{JobInfo, JobState};

use super::{append_job_output, job_finished_message, replace_job, started_job};

fn finished(id: &str, label: &str) -> JobInfo {
    JobInfo {
        id: id.to_owned(),
        label: label.to_owned(),
        script: label.to_owned(),
        state: JobState::Finished,
        exit_code: Some(0),
        timed_out: false,
        elapsed_ms: 1_200,
        output: String::new(),
        dropped: false,
        output_bytes_total: 0,
    }
}

#[test]
fn a_started_job_is_running_and_carries_its_command() {
    let job = started_job("job-1", "cargo build");

    assert_eq!(job.id, "job-1");
    assert_eq!(job.label, "cargo build");
    assert_eq!(job.state, JobState::Running);
}

#[test]
fn a_finished_job_replaces_the_running_one() {
    let mut jobs = vec![started_job("job-1", "cargo build")];

    replace_job(&mut jobs, finished("job-1", "cargo build"));

    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].state, JobState::Finished);
}

#[test]
fn a_job_the_list_does_not_have_is_added() {
    let mut jobs = Vec::new();

    replace_job(&mut jobs, started_job("job-1", "cargo build"));

    assert_eq!(jobs.len(), 1);
}

#[test]
fn the_output_a_job_writes_keeps_what_it_wrote_before() {
    let mut jobs = vec![started_job("job-1", "cargo build")];

    append_job_output(&mut jobs, "job-1", "first\n");
    append_job_output(&mut jobs, "job-1", "second\n");

    assert_eq!(jobs[0].output, "first\nsecond\n");
}

#[test]
fn the_output_of_a_job_the_list_does_not_have_is_dropped() {
    let mut jobs = Vec::new();

    append_job_output(&mut jobs, "job-1", "first\n");

    assert!(jobs.is_empty());
}

#[test]
fn a_job_that_ended_says_how_it_ended() {
    let message = job_finished_message(&finished("job-1", "cargo build"));

    assert!(message.contains("finished"));
    assert!(message.contains("cargo build"));
    assert!(message.contains("exit 0"));
}

#[test]
fn a_job_that_was_stopped_says_so() {
    let mut job = finished("job-1", "cargo build");
    job.state = JobState::Cancelled;

    let message = job_finished_message(&job);

    assert!(message.contains("stopped"));
}
