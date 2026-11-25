//! Running waited and detached child turns.

use async_trait::async_trait;
use norte::{FnExecutor, ToolContext, ToolOutput};

use super::{
    DetachedRun, Steered, SubagentArgs, SubagentOutcome, SubagentRequest, SubagentTool,
    emit_child_update, outcome_to_output, parent_key, report, service_of, started, subagent_label,
};
use norte::Cancellation;
use std::sync::Arc;

impl SubagentTool {
    fn spawn_detached(&self, run: DetachedRun) -> ToolOutput {
        let DetachedRun {
            parent,
            call_id,
            tool_call_id,
            label,
            args,
            cancel,
            token,
        } = run;
        let id = match self
            .jobs
            .try_start(parent, call_id, &label, Arc::clone(&cancel))
        {
            Ok(id) => id,
            Err(error) => {
                self.gate.release_slot();
                self.detaches.release(parent, &token);
                return ToolOutput::error(error.to_string());
            }
        };
        let runner = Arc::clone(&self.runner);
        let jobs = Arc::clone(&self.jobs);
        let steering = Arc::clone(&self.steering);
        let children = Arc::clone(&self.children);
        let service_slot = Arc::clone(&self.service);
        let gate = Arc::clone(&self.gate);
        let request = SubagentRequest {
            prompt: args.prompt,
            parent,
            tool_call_id,
            output_schema: args.output_schema,
            lifetime: args.lifetime,
            agent_id: args.agent_id,
            context_mode: args.context_mode,
        };
        let job_id = id.clone();
        tokio::spawn(async move {
            let outcome = runner.run(request, Arc::clone(&cancel)).await;
            jobs.write_id(&job_id, &outcome.text);
            jobs.finish(&job_id, None);
            steering.push(
                parent,
                Steered::new(
                    format!("subagent:{job_id}"),
                    report(&job_id, &outcome),
                    Vec::new(),
                    Vec::new(),
                ),
            );
            if let Some(service) = service_of(&service_slot) {
                service.schedule_auto_turn(parent);
            }
            emit_child_update(&children, &service_slot, parent, &outcome);
            gate.release_slot();
        });
        self.detaches.release(parent, &token);
        started(&id, &label)
    }
}

#[async_trait]
impl FnExecutor<SubagentArgs, ToolOutput> for SubagentTool {
    async fn call(&self, _args: SubagentArgs) -> norte::Result<ToolOutput> {
        Ok(ToolOutput::error(
            "spawn_agent needs a client attached: a child session belongs to a parent one",
        ))
    }

    async fn call_with_context(
        &self,
        context: &ToolContext,
        args: SubagentArgs,
    ) -> norte::Result<ToolOutput> {
        let (Some(parent), Some(call_id)) = (context.session, context.call_id) else {
            return Ok(ToolOutput::error(
                "spawn_agent needs a client attached: a child session belongs to a parent one",
            ));
        };
        let tool_call_id = call_id.to_string();
        // The delegation depth is the parent's depth plus one (RFC 8.2.4):
        // past the limit the call is an error the model reads, not a child
        // that cannot do anything.
        if let Some(service) = service_of(&self.service)
            && let Some(depth) = parent_key(&service, parent).map(|key| service.depth_of(&key))
        {
            let max_depth =
                u32::try_from(service.delegation_limits().max_depth).unwrap_or(u32::MAX);
            if depth >= max_depth {
                return Ok(ToolOutput::error(format!(
                    "BUDGET_EXCEEDED: maximum delegation depth ({max_depth}) reached at this session"
                )));
            }
        }
        if let Err(message) = self.gate.acquire() {
            return Ok(ToolOutput::error(message));
        }
        let label = subagent_label(&args);
        let cancel = Arc::new(Cancellation::new());
        // The detach token this call watches, published so `Ctrl-G` reaches
        // it, like any other call of the session.
        let token = Arc::new(Cancellation::new());
        self.detaches.attach(parent, Arc::clone(&token));

        if args.detach {
            return Ok(self.spawn_detached(DetachedRun {
                parent,
                call_id,
                tool_call_id,
                label,
                args,
                cancel,
                token,
            }));
        }

        // A waited call remains part of the parent turn and can be detached
        // by the user while it is running.
        let turn_cancelled = context.cancellation();
        let outcome = tokio::select! {
            outcome = self.runner.run(
                SubagentRequest {
                    prompt: args.prompt.clone(),
                    parent,
                    tool_call_id: tool_call_id.clone(),
                    output_schema: args.output_schema.clone(),
                    lifetime: args.lifetime,
                    agent_id: args.agent_id.clone(),
                    context_mode: args.context_mode,
                },
                Arc::clone(&cancel),
            ) => outcome,
            () = token.cancelled() => {
                cancel.cancel();
                SubagentOutcome {
                    text: String::new(),
                    is_error: false,
                    child: None,
                    output_schema_valid: None,
                }
            },
            () = turn_cancelled.cancelled() => {
                cancel.cancel();
                SubagentOutcome {
                    text: String::new(),
                    is_error: true,
                    child: None,
                    output_schema_valid: None,
                }
            },
        };
        self.detaches.release(parent, &token);
        self.gate.release_slot();

        self.emit_child_update(parent, &outcome);
        Ok(outcome_to_output(outcome))
    }
}
