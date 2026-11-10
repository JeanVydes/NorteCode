//! One part of the conversation page.

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::*;

#[component]
pub(super) fn plans_panel() -> Element {
    let context = use_context::<AppContext>();
    let session_id = context
        .selected_session
        .read()
        .as_ref()
        .map(|session| session.session_id.clone())
        .unwrap_or_default();
    let mut plans = use_signal(Vec::new);
    let mut loading = use_signal(|| true);
    use_effect(use_reactive((&session_id,), move |(session_id,)| {
        if session_id.is_empty() {
            return;
        }
        spawn(async move {
            loading.set(true);
            if let Some(client) = api(&context)
                && let Ok(list) = client.plans(&session_id).await
            {
                plans.set(list);
            }
            loading.set(false);
        });
    }));

    let listed = plans.read().clone();
    if *loading.read() {
        return rsx! { p { class: "helper-text", "Loading plans…" } };
    }
    if listed.is_empty() {
        return rsx! {
            EmptyState {
                title: "No plans",
                message: "When the model proposes a plan, it appears here and in the transcript for approval."
            }
        };
    }
    rsx! {
        div { class: "session-list",
            for plan in listed {
                details { class: "tool", key: "{plan.id}",
                    summary { class: "tool-line",
                        span { class: "tool-name", "plan" }
                        span { class: "tool-target",
                            "{plan.title.clone().or_else(|| plan.content.lines().next().map(str::to_owned)).unwrap_or_else(|| plan.id.clone())}"
                        }
                        span { class: plan_status_class(plan.status), "{plan.status.label()}" }
                    }
                    if let Some(model) = &plan.model {
                        p { class: "helper-text", "{model}" }
                    }
                    if *context.show_thinking.read() || plan.status != norte_code_wire::PlanStatus::Pending {
                        div { class: "markdown-body",
                            dangerous_inner_html: crate::components::render_markdown(&plan.content)
                        }
                    }
                }
            }
        }
    }
}

pub(super) fn plan_status_class(status: norte_code_wire::PlanStatus) -> &'static str {
    match status {
        norte_code_wire::PlanStatus::Active => "status status-active",
        norte_code_wire::PlanStatus::Pending => "status status-warning",
        norte_code_wire::PlanStatus::Canceled => "status status-muted",
    }
}

#[component]
pub(super) fn jobs_panel() -> Element {
    let context = use_context::<AppContext>();
    let session_id = context
        .selected_session
        .read()
        .as_ref()
        .map(|session| session.session_id.clone())
        .unwrap_or_default();
    // The live stream keeps running jobs current; asking the host once on open
    // fills in the ones that finished before this page was watching.
    use_effect(use_reactive((&session_id,), move |(session_id,)| {
        if session_id.is_empty() {
            return;
        }
        spawn(async move {
            if let Some(client) = api(&context)
                && let Ok(jobs) = client.list_jobs(&session_id).await
            {
                context.set_jobs(jobs);
            }
        });
    }));
    let jobs = context.jobs.read().clone();
    if jobs.is_empty() {
        return rsx! {
            EmptyState { title: "No jobs", message: "Detached tool calls show up here while they run." }
        };
    }
    rsx! {
        div { class: "job-list",
            for job in jobs {
                details {
                    class: if job.status == norte_code_wire::JobStatus::Running { "tool job job-running" } else { "tool job" },
                    key: "{job.job_id}",
                    open: job.status == norte_code_wire::JobStatus::Running,
                    summary { class: "tool-line",
                        span { class: "tool-name", "shell" }
                        span { class: "tool-target", "{job.command}" }
                        if let Some(ms) = job.duration_ms {
                            span { class: "tool-time", "{ms} ms" }
                        }
                        span {
                            class: if job.status == norte_code_wire::JobStatus::Running {
                                "tool-state tool-state-running"
                            } else {
                                "tool-state tool-state-done"
                            },
                            "{job.status.label()}"
                        }
                    }
                    // The tail is the point: a running job streams its output,
                    // and the panel follows it to the end as it grows.
                    if let Some(output) = &job.output_snapshot {
                        pre { class: "tool-output job-tail", "{output}" }
                    } else {
                        p { class: "helper-text", "No output yet." }
                    }
                    if job.output_truncated {
                        p { class: "helper-text", "Earlier output was truncated." }
                    }
                }
            }
        }
    }
}
