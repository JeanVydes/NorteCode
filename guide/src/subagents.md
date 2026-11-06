# Subagents

The model can delegate one turn to a child session with the `subagent`
tool. The child runs one turn and answers; the model reads back what it
said.

- The child starts **blank**. The prompt carries everything it needs —
  there is no shared context with the parent.
- A child **never has `subagent`**, so nesting stops at one. A delegation
  is one level deep by construction, not by a flag a child could flip.
- The child inherits the **parent's provider and model** when the agent's
  `subagent` field is `null`. Set it to another configured agent name to use
  that agent's defaults; the name may also refer to the parent agent.

## Detaching

With `detach: true` the call becomes a job (`job-N`), like a detached
shell. It returns the job id at once, the run continues, `manage_jobs`
reads and stops it, and its result arrives as a message when it ends.

## Cost and trust

Every child spends tokens on its own turn. The tool is enabled unless the
agent lists `subagent` in `tools.disabled`; it is capped at three concurrent
runs. Configure the target agent in the `agents` array of `service.json`.
