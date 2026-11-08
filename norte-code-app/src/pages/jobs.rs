use crate::api::{ApiClient, JobSummary};
use crate::components::{EmptyState, PageHeader};
use crate::state::AppContext;
use dioxus::prelude::*;

#[component]
pub fn JobsPage() -> Element {
    let context = use_context::<AppContext>();
    use_effect(move || {
        let selected_session = context
            .selected_session
            .read()
            .as_ref()
            .map(|session| session.session_id.clone());
        if selected_session.is_some() {
            refresh_jobs(&context);
        }
    });
    let jobs = context.jobs.read().clone();
    let selected = context.selected_session.read().clone();
    rsx! {
        PageHeader {
            eyebrow: "Operations",
            title: "Jobs",
            description: "Inspect and cancel the server's basic session jobs. Job state remains owned by the RAEP host."
        }
        div { class: "section-heading",
            h2 { "{selected.as_ref().map_or(\"No session selected\", |session| session.title.as_deref().unwrap_or(\"Untitled session\"))}" }
            button {
                class: "button-secondary",
                disabled: selected.is_none(),
                onclick: move |_| refresh_jobs(&context),
                "Refresh jobs"
            }
        }
        if selected.is_none() {
            EmptyState {
                title: "Choose a session",
                message: "Select a session first; the RAEP job query is scoped to one session."
            }
        } else if jobs.is_empty() {
            EmptyState {
                title: "No jobs loaded",
                message: "Refresh to query this session's jobs. The server may return an empty list."
            }
        } else {
            div { class: "job-grid",
                for job in jobs {
                    JobCard { job }
                }
            }
        }
    }
}

#[component]
fn JobCard(job: JobSummary) -> Element {
    let context = use_context::<AppContext>();
    let job_for_cancel = job.clone();
    let can_cancel = job.status == crate::api::JobStatus::Running;
    let label = if job.command.is_empty() {
        "Unlabeled job".to_owned()
    } else {
        job.command.clone()
    };
    rsx! {
        article { class: "job-card",
            div { class: "job-card-head",
                div {
                    h3 { "{label}" }
                    p { class: "mono", "{job.job_id}" }
                }
                span { class: job.status.css_class(), "{job.status.label()}" }
            }
            if let Some(exit_code) = job.exit_code {
                p { class: "session-meta", "Exit code: {exit_code}" }
            }
            if let Some(output) = job.output_snapshot {
                pre { class: "output-box", "{output}" }
            }
            if can_cancel {
                div { class: "button-row", style: "margin-top: 1rem",
                    button {
                        class: "button-danger",
                        onclick: move |_| cancel_job(&context, job_for_cancel.clone()),
                        "Cancel job"
                    }
                }
            }
        }
    }
}

fn refresh_jobs(context: &AppContext) {
    let context = *context;
    context.set_jobs(Vec::new());
    spawn(async move {
        context.set_page_error(None);
        let Some((api, session_id, agent_id)) = active_session(&context) else {
            return;
        };
        if let Err(error) = api.attach(&session_id, &agent_id).await {
            context.set_page_error(Some(error.to_string()));
            return;
        }
        let jobs = api.list_jobs(&session_id).await;
        let detach = api.detach(&session_id).await;
        match (jobs, detach) {
            (Ok(jobs), Ok(())) => context.set_jobs(jobs),
            (Err(error), _) | (Ok(_), Err(error)) => {
                context.set_page_error(Some(error.to_string()));
            }
        }
    });
}

fn cancel_job(context: &AppContext, job: JobSummary) {
    let context = *context;
    spawn(async move {
        context.set_page_error(None);
        let Some((api, session_id, agent_id)) = active_session(&context) else {
            return;
        };
        let result = async {
            api.attach(&session_id, &agent_id).await?;
            let cancel = api.cancel_job(&session_id, &job.job_id).await;
            let detach = api.detach(&session_id).await;
            match (cancel, detach) {
                (Err(error), _) | (Ok(()), Err(error)) => Err(error),
                (Ok(()), Ok(())) => Ok(()),
            }
        }
        .await;
        match result {
            Ok(()) => refresh_jobs(&context),
            Err(error) => context.set_page_error(Some(error.to_string())),
        }
    });
}

fn active_session(context: &AppContext) -> Option<(ApiClient, String, String)> {
    let Some(profile) = context.active_profile() else {
        context.set_page_error(Some("Select a server profile first.".to_owned()));
        return None;
    };
    let Some(session) = context.selected_session.read().clone() else {
        context.set_page_error(Some("Choose a session first.".to_owned()));
        return None;
    };
    let agent_id = session.agent.map_or_else(String::new, |agent| agent.id);
    match ApiClient::new(&profile.base_url, &profile.bearer_token) {
        Ok(api) => Some((api, session.session_id, agent_id)),
        Err(error) => {
            context.set_page_error(Some(error.to_string()));
            None
        }
    }
}
