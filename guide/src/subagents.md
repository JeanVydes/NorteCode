# Subagents

The model can delegate one turn to a child session with the `subagent`
tool. The child runs one turn and answers; the model reads back what it
said.

- The child starts **blank**. The prompt carries everything it needs —
  there is no shared context with the parent.
- A child **never has `subagent`**, so nesting stops at one. A delegation
  is one level deep by construction, not by a flag a child could flip.
- The child inherits the **parent's model** (it already has credentials
  for it). `provider` and `model` can ask for another one.

## Detaching

With `detach: true` the call becomes a job (`job-N`), like a detached
shell. It returns the job id at once, the run continues, `manage_jobs`
reads and stops it, and its result arrives as a message when it ends.

## Cost and trust

Every child spends tokens on its own turn. The tool is off by default
and capped at three concurrent runs — past the cap a spawn is an error
the model reads, not a queue. Configure it under `tools.subagent`.
