# Storage and durability

## One SQLite file

The CLI stores everything in:

```text
$XDG_DATA_HOME/norte-code/sessions.db
# or ~/.local/share/norte-code/sessions.db
# or .norte-code/sessions.db when neither variable is set
```

The file is not a cache. It contains the conversation, the host's bookkeeping
and the audit trail.

## What is stored

| Table | Contents |
| --- | --- |
| `sessions`, `nodes` | Norte's own session and commit tree. |
| `blobs` | Norte blobs, including media. |
| `logical_sessions` | Client key, current Norte session, title, model, phase, stats, rules and compaction link. |
| `approvals` | Durable parked approvals and their grants. |
| `questions` | Pending questions, their options and answers. |
| `effects` | Intent/finish journal for effectful tools. |
| `costs` | One row per billed request and the provider-reported amount. |
| `requests` | One row per provider attempt, including failures, for telemetry. |
| `settings` | Host settings. |

The store uses WAL, foreign keys and one transaction per Norte commit. SQLite is
synchronous, so calls run on the blocking pool behind one connection; that is
appropriate for a desktop host, not a multi-tenant server.

## Optimistic concurrency

A Norte commit carries the version it was built from. The store compares and
swaps: if another writer moved the session, the commit fails instead of
overwriting it. That is what makes two workers safe to run against one session.

Norte guarantees commit-at-most-once, not effect-at-most-once. A process can die
after a command ran but before the commit landed. The effect journal closes that
gap:

1. Write an intent before an effectful executor runs.
2. Mark it finished from the tool event.
3. On startup, a call still at intent is refused rather than replayed.

The journal covers `write`, `edit`, `command` and `skill_script`.

## Repair on load

A session left in `Streaming` by a hard kill is not actually streaming anymore.
On the next load the service puts the Norte store back to `Idle` and rewrites
the directory row. Approvals and questions are left parked; they are decisions
for the human, not stale locks.

## Resume

A parked session can be resumed in a new process because approvals, grants,
questions and effects are in SQLite. `norte-code sessions` lists the keys and
phases; `norte-code resume <key>` continues. A session stopped by a budget or by
Ctrl-C is resumable in the same way.

## Deletion

There is no retention policy and no cloud copy. Deleting `sessions.db` deletes
the local sessions, costs, telemetry and pending decisions. Back it up like any
other local database.
