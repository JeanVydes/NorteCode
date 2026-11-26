# Plugins

A plugin is a shared library (`.so` / `.dll` / `.dylib`) that adds things to
Norte Code without changing it: tools, slash commands, widgets, and providers and models in the catalogue. The
[catalog-plugin](https://github.com/JeanVydes/catalog-plugin) is the
ready-made example: it offers a catalogue so you do not write
`providers.json` and `models.json` by hand.

## What a plugin can do

The plugin trait defines these surfaces:

| Surface | What it adds |
| --- | --- |
| `name()` | The name reported in `/plugins`, in load errors and as the origin of its tools. |
| `tools()` | Norte `AnyFunction` values the host registers with the tool registry. |
| `widgets()` | Stable widget IDs and titles that clients can discover. |
| `widget(id, selection)` | JSON content for a widget the client requested. |
| `register_commands()` | Slash commands with typed arguments and completion. |
| `catalogue()` | Providers and models the catalogue knows for that run. |

A tool declared by a plugin is a normal tool from the model's point of view.
The host owns registration, so the tool gets the same schema handling,
approval flow and audit record as a built-in tool. `tools.disabled` applies to
plugin tools too. The tool's name is listed in `/plugins`, and a call shows
which plugin declared it in the transcript.

A widget can publish text, progress, lists, key-value fields, tables, badges,
sparklines and separators. Its content does not specify a position or size.
The TUI converts it into terminal components and places its chosen widgets
inside the chat rail. Other clients can choose their own placements.

The selection supplied to `widget()` identifies the caller's session, provider
and model. It may describe a draft model before a session exists. Return
`None` when the widget is unavailable for that selection. Keep the context
received in `set_context()` and call `request_widget_update(&id)` when a
registered widget changes. The host wakes only subscriptions to that widget.
Slash command execution receives the invoking session's selection.

## What a plugin cannot do

A plugin cannot write to the terminal. The API does not give it a terminal, so
it cannot clear a line it does not own or fight the frame for a row.

A plugin cannot block. Trait methods return data, and a tool's `execute`
future is created by the plugin and polled by the host. Any timer or I/O that
depends on the plugin's own runtime belongs in a task the plugin owns.

A plugin cannot register tools itself, change approval rules, edit prompts or
configuration, or remove built-in tools. It only declares data, and the host
decides what to do with it.

## Plugin user messages

A plugin can ask the host to add a user-role message to a conversation. The
plugin never writes history itself. It attaches a reserved
`norte_code.host_effects` envelope to `ToolOutput.structured`. The host removes
that envelope before the result is stored, shown or sent to a model, then
validates the request against global and per-plugin switches before committing
a message.

Only text and media parts are accepted. The host checks model modalities,
converts inline media to blobs and records provenance as a `Part::Extension` in
the `norte-code/plugin-message` namespace. A plugin without explicit permission
has its envelope discarded, and the host reports the refusal as a
`plugin_message_rejected` event. A tool that wants to use this envelope must
return a `ToolOutput` from a `ToolDeclaration`, because a `FunctionDeclaration`
wraps the result again and hides the envelope from the host.

The browser plugin wraps its tool so a screenshot also produces a PNG message
draft. The tool result still reports only the byte count, and the host drops the
image when the model has no image input. This stays off until an agent opts in:

```json
{
  "tools": {
    "plugin_messages": {
      "enabled": true,
      "allow_media": true,
      "max_messages_per_turn": 1,
      "max_bytes": 1048576
    }
  },
  "plugins": {
    "browser": { "allow_user_messages": true }
  }
}
```

## Loading and lifecycle

A plugin that fails to load is reported and skipped. The server starts either
way. The host checks the library's `norte_plugin_abi_version` against its own
before loading it. The current interface is ABI version 4. A mismatch is
refused with both numbers named, never loaded on a guess.

Plugins are loaded once at server startup. Tools, commands and catalogue entries are registered then. Widgets are evaluated on demand. Install, update, remove, enable and disable
change files and emit `PluginChanged`, but they do not load or unload a shared
library in the running process. Restart the service to apply any of them:

```text
norte service restart
```

Until a restart, a plugin disabled after it loaded still reports as loaded,
and its tools and views can still be supplied by that process. There is
no explicit plugin reload request.

## The catalogue plugin, in detail

`catalog-plugin` registers providers and models through `catalogue()`. Its providers and
models are merged into the catalogue **for that run**; the files you own
are not written and do not have to exist. That is the whole point: a
plugin that serves a model the catalogue does not know offers it there,
and it is selectable in `/model` like any other.

The Codex plugin displays the available account usage windows when a model
from its catalogue is selected. It reads them through the local Codex CLI
on the Norte Code server. That CLI must be installed and signed in.
