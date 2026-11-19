# norte-code-tui

The Norte Code terminal interface, built on [terminalui](https://github.com/JeanVydes/terminalui).
This is the client the `norte` binary delegates to.

It talks to the raep server and owns the screen: a retained-mode tree of
typed views, central focus management, and a strict raep state machine
that never pretends to be attached when it is not.

Highlights:

- Every catalogue view: sessions, models, plans, jobs, telemetry, host,
  plugins, costs, live status, themes.
- Chat with honest connection state, Ctrl+R to re-attach, Ctrl+T to switch themes.
- Models page with provider expansion and key prompts for providers that need one.
- Ask-user overlay for questions the host parks a turn on.
- Alt+1..9 to switch tabs, multiline composer, mouse and keyboard both.

## Theme config

`cli.json` under `~/.config/norte-code/` can select a theme and declare more:

```json
{
  "service": { "url": "http://127.0.0.1:8787" },
  "ui": {
    "theme": "ocean",
    "themes": [
      { "name": "ocean", "base": "dark", "palette": { "accent": "#00aaff" } }
    ]
  }
}
```

`base` is one of `dark`, `cegador`, `pastel_hueso`, `amoled`. Color roles take
`#RRGGBB` or `RRGGBB`; missing or invalid roles keep the base color.
Ctrl+T cycles the built-ins followed by the configured themes.
