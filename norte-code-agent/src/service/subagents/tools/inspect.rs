//! Child inspection tools.

use std::sync::Arc;

use async_trait::async_trait;
use norte::{FnExecutor, ToolContext, ToolOutput};
use norte_code_wire::SessionKey;

use super::super::{
    InspectArgs, InspectViewKind, RwLock, Service, SharedChildren, answer, child_key, clip,
    parent_key, phase_of, registered_child_state, service_of,
};
use crate::service::subagent_views::{ChildStatus, InspectView, JobView};

/// `inspect_agent` (RFC 8.2.7): look at a child the parent spawned.
pub(crate) struct InspectAgent {
    children: SharedChildren,
    service: Arc<RwLock<Option<Arc<Service>>>>,
}

impl InspectAgent {
    /// An inspector over the shared child registry.
    pub(crate) fn new(
        children: SharedChildren,
        service: Arc<RwLock<Option<Arc<Service>>>>,
    ) -> Self {
        Self { children, service }
    }

    /// Turns the tool into a declaration the core registry accepts.
    pub(crate) fn into_declaration(self) -> norte::ToolDeclaration<InspectArgs> {
        norte::ToolDeclaration::new(
            "inspect_agent",
            "Look at a child session you spawned: its state and what it last said. `view: status` (the default) is the one to reach for in a turn; `transcript` returns its history, `jobs` its background jobs.",
            Arc::new(self),
        )
    }
}

#[async_trait]
impl FnExecutor<InspectArgs, ToolOutput> for InspectAgent {
    async fn call(&self, _args: InspectArgs) -> norte::Result<ToolOutput> {
        Ok(ToolOutput::error("inspect_agent needs a session"))
    }

    async fn call_with_context(
        &self,
        context: &ToolContext,
        args: InspectArgs,
    ) -> norte::Result<ToolOutput> {
        let (Some(parent), Some(service)) = (context.session, service_of(&self.service)) else {
            return Ok(ToolOutput::error(
                "inspect_agent needs a client attached: a child belongs to a parent session",
            ));
        };
        let Some(parent_key) = parent_key(&service, parent) else {
            return Ok(ToolOutput::error(
                "inspect_agent: parent session is unknown",
            ));
        };
        let Ok(child) = child_key(&args.child_session_id) else {
            return Ok(ToolOutput::error("inspect_agent needs a child_session_id"));
        };
        let view = args.view.unwrap_or_default();
        match view {
            InspectViewKind::Status => {
                let Some(registered) = self.children.get(&parent_key, &child) else {
                    return Ok(ToolOutput::error(format!(
                        "CHILD_NOT_FOUND: no child `{}` under this session; use `list_agents`",
                        args.child_session_id
                    )));
                };
                let phase = phase_of(&service, &child);
                let state = registered_child_state(&registered, phase);
                let title = service
                    .lock_sessions()
                    .get(&child)
                    .and_then(|record| record.title.clone())
                    .unwrap_or_default();
                Ok(ToolOutput::text(answer(&InspectView::Status {
                    child_session_id: child.as_str(),
                    status: ChildStatus {
                        state,
                        title,
                        delegator_suspended: registered.suspended,
                    },
                })))
            }
            InspectViewKind::Transcript => Ok(inspect_transcript(&service, &child, &args).await),
            InspectViewKind::Jobs => Ok(inspect_jobs(&service, &child, &args)),
            InspectViewKind::Events => Ok(ToolOutput::text(answer(&InspectView::Events {
                child_session_id: child.as_str(),
                items: Vec::new(),
                truncated: false,
                note: "the event view reads the RAEP replay window, which the service does not expose to the model yet",
            }))),
        }
    }
}

async fn inspect_transcript(
    service: &Service,
    child: &SessionKey,
    args: &InspectArgs,
) -> ToolOutput {
    let (nid, spec, _) = match service.require_record(child) {
        Ok(found) => found,
        Err(error) => return ToolOutput::error(format!("CHILD_NOT_FOUND: {error}")),
    };
    let Some(spec) = spec else {
        return ToolOutput::error("inspect_agent: child has no model");
    };
    let engine = match service.engine(&spec, Some(child)) {
        Ok(engine) => engine,
        Err(error) => return ToolOutput::error(format!("inspect_agent: {error}")),
    };
    let history = match engine.history(nid).await {
        Ok(history) => history,
        Err(error) => return ToolOutput::error(format!("inspect_agent: {error}")),
    };
    let limit = args.limit.unwrap_or(20).clamp(1, 200);
    let from = usize::try_from(args.from_seq.unwrap_or(0)).unwrap_or(usize::MAX);
    let items: Vec<String> = history
        .iter()
        .skip(from)
        .take(limit)
        .map(|message| {
            let text = norte::Message::text(message);
            format!("{:?}: {}", message.role, clip(&text, 400))
        })
        .collect();
    ToolOutput::text(answer(&InspectView::Transcript {
        child_session_id: child.as_str(),
        truncated: history.len() > from.saturating_add(items.len()),
        items,
    }))
}

fn inspect_jobs(service: &Service, child: &SessionKey, args: &InspectArgs) -> ToolOutput {
    let nid = match service.require_record(child) {
        Ok((nid, _, _)) => nid,
        Err(error) => return ToolOutput::error(format!("CHILD_NOT_FOUND: {error}")),
    };
    let jobs = service.jobs.list(Some(nid));
    let limit = args.limit.unwrap_or(20).clamp(1, 100);
    let jobs = jobs
        .iter()
        .take(limit)
        .map(|job| JobView {
            job_id: &job.id,
            command: &job.label,
            status: job.state.label(),
        })
        .collect();
    ToolOutput::text(answer(&InspectView::Jobs {
        child_session_id: child.as_str(),
        jobs,
    }))
}
