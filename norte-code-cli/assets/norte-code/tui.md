# TUI preferences and reference corrections

The user `cli.json` contains a `ui` object. Settings changes persist here. Other top-level fields belong to the CLI runtime and must be preserved. The TUI loads the user file from `$XDG_CONFIG_HOME/norte-code/cli.json`, or `~/.config/norte-code/cli.json` when XDG is unset. It does not currently load a project `cli.json` overlay.

| Field | Type and default | Effect |
| --- | --- | --- |
| `theme` | string or null | Built-in or custom theme name. |
| `themes` | array | Custom themes; inspect existing entries before editing. |
| `rail_position` | `left`, `right`, `top`, `bottom`, `hide` | Position or hide the session rail. |
| `debug` | boolean, false | Show performance overlay. |
| `show_tool_output` | boolean, false | Default expansion of tool details and shell output; individual calls can be toggled. |
| `tool_verbosity` | boolean, false | Group reasoning and tool activity into expandable blocks. |
| `chart_density` | integer 2 through 6, default 4 | Chart glyph density. |
| `chart_contrast` | number 0.25 through 4.0, default 1.0 | Chart glyph contrast; rounded to quarter steps. |
| `chart_dot_size` | `small`, `medium`, `large`; default `medium` | Halftone dot glyph size. |
| `reasoning` | `auto`, `low`, `medium`, `high` | Default reasoning selection. |
| `send_mode` | `steer`, `queue`, `interrupt`, `reject` | Behavior when sending during a turn. |
| `diff_mode` | `unified`, `split` | Edit diff layout. |

The reference `configuration.md` describes the broader schema, but its introduction incorrectly states that the running server searches parent directories for project configuration and that the TUI loads project `cli.json`. The server reads `<workspace>/.norte-code/service.json` only. Its JSON object overlays the global service config; arrays replace the global array. The `charts` block described in that reference is distinct from the TUI's `ui.chart_*` values.

Connection has two shapes: `service_url` and `service_auth` for the CLI and, optionally, `service.url` and `service.token` for the TUI. When the nested `service` exists, the TUI uses it. Keep both consistent when editing them. The fields `norte_code_skill`, `service_lifecycle`, `reconnect`, and `retention` are top-level, not inside `ui`. `norte_code_skill: false` removes the managed skill the next time the CLI runs.

`skills.honor_allowed_tools` exists in the schema but has no observed enforcement in the current implementation. Do not promise that it grants permissions. `tools.screenshots` is obsolete and has no effect. Changes to a server file require the service to reload the relevant configuration or restart; CLI configuration is read when the client starts.
