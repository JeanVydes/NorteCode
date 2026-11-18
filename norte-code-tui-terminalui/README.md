# norte-code-tui (mockup)

A visual mockup of NORTE, a mission control for an AI agent, built with
[terminalui](https://github.com/JeanVydes/terminalui). Not a port of the
Norte Code TUI: an original layout, structured to be fast to scan and
honest about state. No agent logic; the turns are scripted and the
connection to the raep is simulated.

Layout, left to right:

- **Session rail.** Agent, model and session identity, an eased context
  gauge with the compaction marker, the plan checklist and the files the
  agent touched. The connection line shows `attached`, `reconnecting` with
  attempt counter, or `failed` with the reason and a retry hint.
- **Conversation stream.** A windowed, height-cached transcript: only the
  visible rows render, so a long history scrolls without flicker. User
  messages carry their accent bar; tool calls show icon, name, target and
  status; edits render as colored diff rows; shells run in a real
  terminalui `Console`. A thinking row shimmers while the agent works.
- **Multiline composer.** A real terminalui `Input`: multiline, caret
  pulse, placeholder, Enter newlines, `Ctrl+Enter` sends.
- **Status strip.** Selectors for queue mode and reasoning (cycle with the
  mouse, `Tab`, `Left`/`Right` or `Ctrl+Q`/`Ctrl+R`), the send hint and
  the turn state.

The client refuses optimistic behavior: with the raep unreachable it
refuses to send the message, surfaces the error in the stream and offers
an explicit retry. Dropping the connection and hard failures are part of
the demo, not accidents.

Keys:

| Key | Action |
|---|---|
| `F1` | help overlay |
| `Ctrl+Enter` | send |
| `Tab` / `Shift+Tab` | cycle focus |
| `PgUp` / `PgDn` | scroll the stream |
| `Ctrl+T` | cycle theme: dark, cegador, pastel hueso, amoled |
| `Ctrl+R` / `Ctrl+Q` | reasoning / queue mode |
| `Ctrl+E` | simulate connection drop |
| `Ctrl+F` | simulate raep unreachable |
| `Ctrl+X` | retry: rerun tests or reconnect |
| `Ctrl+C` / `Esc` | quit |

Run it:

```console
cargo run
```

Requirements: Rust 1.94 or newer, a truecolor terminal. The themes were
checked at 110x34 and 80x28; everything clips rather than crashes below
that.
