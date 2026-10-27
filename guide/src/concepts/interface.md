# The interface

How the client draws. This is the design the interface is being moved onto, one
widget at a time; what is written here is the target, and the sections say which
part is still where it was.

## The problem it replaces

Everything on screen used to be written as it was produced. The renderer printed
a tool's output as the events arrived, the editor painted its own block by moving
the cursor to absolute rows, and the loading bar repainted a line of its own from
a thread. Each writer kept its own idea of where the cursor was — `last_char`,
`output_line_start`, `reasoning_line_start`, `painted` — and each of those is a
promise about the terminal that the next writer can break.

The bugs are all the same bug. A question box fell out of its block because two
writers counted rows differently. The footer showed an effort the session was not
on because the value came from two places. Output could not be redrawn because
the rows it occupied were not owned by anyone. Hand-written escapes moved the
cursor by a count that had to stay true, and nothing checked that it did.

A frame cannot have that problem: there is one buffer, the widgets fill it, and
the terminal is written once.

## The shape

```text
┌─────────────────────────────────────────────┐
│ Transcript                                  │  Fill — the conversation,
│                                             │  scrolled by the app
├─────────────────────────────────────────────┤
│ Input                                       │  Min(1), Max(6) — the draft,
│                                             │  or the question
├─────────────────────────────────────────────┤
│ Status                                      │  Length(1) — model, dial,
│                                             │  cost, effort
├─────────────────────────────────────────────┤
│ Tabs                                        │  Length(1) — the row and what
│                                             │  the active tab shows
├─────────────────────────────────────────────┤
│ Suggestion                                  │  Length(1) — the completion
└─────────────────────────────────────────────┘
```

Measured from the top down, in constraints, not in rows counted from the bottom.
That is the whole change: the transcript takes what is left rather than the
chrome reserving fixed rows from the bottom and the transcript living in whatever
the terminal left over.

## The rules

**State is not a widget.** The transcript is a `Vec<Entry>`, the draft is a
`LineBuffer`, the cursor position is an index. A widget borrows all of it and
writes nothing: it takes `&State` and returns nothing, and the one place that
owns the terminal draws the result. Nothing in a widget reads the clock, reads a
file, or waits — a widget that could block would block the frame.

**One owner of the terminal.** `terminal::Guard`. The editor, the menu, the
approval panel and the loading bar all draw through it. Nothing else writes to
stdout or stderr while a frame is up.

**A frame per change, not per clock.** The event loop blocks on its channel and
draws when something arrived. The one thing that needs a clock is the spinner,
and it is a tick that only redraws while a turn is in flight: an idle client
draws nothing at all.

**Everything that is bounded says so.** A list that hid rows without counting
them is a list a person cannot tell from a complete one. Every truncation in the
interface names what it dropped — the tool output box, the tab summaries, the
completion menu, the question's choices.

**Width is not assumed.** No widget holds a number of columns. Text is wrapped by
the layout, and the wrap is what the row count comes from, so a resize is one
more frame and never a migration.

## What each part is

### Transcript

The conversation, oldest at the top: user messages, the answer, the reasoning
gutter, tool calls with their output boxes, diffs, the loading bar while a turn
runs.

It is the one part that is not a fixed-height widget. Two shapes are possible and
the choice is still open:

- **A view over the entries.** The app keeps the transcript and draws the part
  that fits, with PgUp/PgDn and a follow-the-tail flag. Everything is the app's,
  including the scrollback, which is what makes a resize and a re-render exact.
- **A pager into the terminal's own scrollback.** Left as it was, with the
  transcript in the region above the chrome. Cheaper, and it is what a terminal
  is for — but it is the thing that cannot be composed, redrawn or resized.

**Chosen: the view.** The why is the state more than the drawing. The client
already receives every event it draws, so the conversation is something it has;
keeping it that way round makes the scroll position a value the app owns, and
makes a resize a re-wrap instead of a guess. The pager shape would keep the one
property this change exists to remove — the interface depending on what the
terminal still remembers — and it is the shape that cannot say what it dropped.

Today the transcript does not exist as data: the renderer holds an `answer_buffer`
for the answer in flight and prints every other event as it arrives, at seventy
sites, keeping none. Turning those into entries is the work of this phase, and it
is the larger half of the change.

Entries are stored as they arrived, not as rendered lines, whichever shape is
chosen: a resize re-wraps them, and a re-wrap of text that was already wrapped
is words split in the wrong places.

### Input

The draft, the prompt, the completion menu, and the question when the model asks
one. All four live in the same rows, so they are one widget with four states and
not four widgets fighting for the same space.

The draft keeps its own buffer — the chips that stand for a pasted paragraph or a
pasted path are this client's, not any text box's — and the widget only draws it.
The menu is drawn inside the input's own rows, which is why the input is
`Min(1) + Max(6)` and not a fixed six: the block grows to fit what is in it.

### Status

One row: the model, the context dial, the cache share, the cost, the reasoning
effort. It is the row the footer has always carried, and it stays a row.

### Tabs

The tab row and the summary of the active tab, on one line. Each tab is a summary
and not a list — a tab has one row — and the lists are commands (`/tools`, `/mcp`)
that print into the transcript.

### Suggestion

The completion hint, on its own row rather than appended to the draft's: it is
about what may be typed next, and it was drawn at the end of the cursor row only
because that row was the only thing the editor had.

## Where a plugin enters

A plugin adds a **panel**: a widget with a name, a place in the layout (above the
status, below the transcript, beside the tabs) and a size constraint. The registry
is the same shape as the command registry — declared once, read by the layout —
and a panel is given only its area and a read-only view of the state.

What a plugin cannot do is as much the point: it cannot write escapes, cannot
move the cursor, cannot claim rows it was not given, and cannot block the frame.
The layout decides, and an area is all it gets.

## What is left to move

| Part | State |
| --- | --- |
| The terminal, the guard, the backend | Done |
| The startup menu | Done — a widget over a pure `Menu` |
| The layout, and the widgets above | Not started |
| The editor: input, footer, tabs | Not started — still escapes and row arithmetic |
| The transcript | Not started — still the terminal's scroll region |
| The approval panel, the loading bar | Not started — still `eprintln!` and a repaint thread |
