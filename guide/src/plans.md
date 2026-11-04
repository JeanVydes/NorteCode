# Plans

A plan is a work proposal the model puts to you before it starts: it saves the
proposal as a Markdown file, asks for your approval, and only an approved plan
becomes `active`. Plans live in the session's folder at
`~/.local/norte-code/<session>/plans/<id>.md` — plain Markdown with YAML front
matter, so you can open and edit them with any editor.

## The five actions

The model calls them through the `plan` tool:

| Action | What it does |
| --- | --- |
| `propose` | Save a new plan as `pending` and ask for approval. `Approve` activates it; `Reject` cancels it. |
| `revise` | Replace an existing plan's content (same id), back to `pending` with `revision` incremented, and ask again. |
| `list` | Show each plan's front matter (id, status, title, timestamps), without bodies. |
| `get` | Read one plan in full. |
| `set_status` | Move a plan to `pending` or `canceled`. |
| `delete` | Remove a plan file. |

The model **cannot** activate a plan on its own: `set_status` refused
`active` — only your approval does that. That is the point of the tool: a
proposal the model cannot approve to itself.

## What you see

In the interactive prompt, `propose`/`revise` open the proposal in a modal —
the exact saved file content — and ask `Approve this plan?`. The file is
saved **before** the question, so if you close the client or the request
fails, the plan stays `pending` on disk and nothing is lost.

Without an interactive client, the proposal is printed to stderr and the plan
stays `pending`.

## Managing plans

- `/plans` in the prompt lists the session's saved plans: status, title,
  path, creation and modification times, and who created them (provider/model).
- Files are yours to edit: change the body, and the next `get`/`list` sees it.
  The front matter (`status`, `title`, `created_at`, `updated_at`, `provider`,
  `model`, `revision`) is what the tool reads back.
- A plan left `pending` (headless run, error, unanswered question) is not
  special: edit the file or ask the model to `revise` it in a later turn of
  the same session.

## Limits

- A plan is a marker, not a gate. Nothing enforces that an `active` plan is
  followed, or blocks tools when no plan is active.
- There is no update in place besides `revise`: to change a plan you revise
  it (same id, new revision, new approval), or delete and propose again.
- `propose`/`revise` wait for your answer. Other plans in the session do not
  change that; each proposal is approved on its own.
