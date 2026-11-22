# Plugins

A plugin is a shared library (`.so` / `.dll` / `.dylib`) that adds things to
Norte Code without changing it: tools the model can call, a status line field,
a banner line, or providers and models in the catalogue. The
[catalog-plugin](https://github.com/JeanVydes/catalog-plugin) is the
ready-made example: it offers a catalogue so you do not write
`providers.json` and `models.json` by hand.

## What a plugin can do

One trait, five optional surfaces, all default to nothing:

| Surface | What it adds |
| --- | --- |
| `name()` | The name reported in `/plugins`, in load errors and as the origin of its tools. |
| `tools()` | Norte `AnyFunction` values the host registers with the tool registry. |
| `status()` | One line in the plugin status section of the sidebar. |
| `banner()` | One line in the plugin banner section of the sidebar. |
| `catalogue()` | Providers and models the catalogue knows for that run. |

A tool declared by a plugin is a normal tool from the model's point of view.
The host owns registration, so the tool gets the same schema handling,
approval flow and audit record as a built-in tool. `tools.disabled` applies to
plugin tools too. The tool's name is listed in `/plugins`, and a call shows
which plugin declared it in the transcript.

`set_context()` is how the host gives the plugin a way to ask about the
session. The host passes a `PluginHost`, and the plugin may keep it and ask
later. The answer follows the most recently attached session: its id, title,
model, provider and context usage. Before any session is attached, the model
and provider fall back to the configured default. The session can move, for
example after a compaction, so ask when needed instead of caching one key.

## What a plugin cannot do

A plugin cannot write to the terminal. The API does not give it a terminal, so
it cannot clear a line it does not own or fight the frame for a row.

A plugin cannot block. Trait methods return data, and a tool's `execute`
future is created by the plugin and polled by the host. Any timer or I/O that
depends on the plugin's own runtime belongs in a task the plugin owns.

A plugin cannot register tools itself, change approval rules, edit prompts or
configuration, or remove built-in tools. It only declares data, and the host
decides what to do with it.

## Loading and lifecycle

A plugin that fails to load is reported and skipped. The server starts either
way. The host checks the library's `norte_plugin_abi_version` against its own
before loading it. The current interface is ABI version 1. A mismatch is
refused with both numbers named, never loaded on a guess.

Plugins are loaded once at server startup. Tools, catalogue entries, status
and banner are captured then. Install, update, remove, enable and disable
change files and emit `PluginChanged`, but they do not load or unload a shared
library in the running process. Restart the service to apply any of them:

```text
norte service restart
```

Until a restart, a plugin disabled after it loaded still reports as loaded,
and its startup snapshot still supplies its tools, status and banner. There is
no explicit plugin reload request.

## The catalogue plugin, in detail

`catalog-plugin` uses only the `catalogue()` surface. Its providers and
models are merged into the catalogue **for that run**; the files you own
are not written and do not have to exist. That is the whole point: a
plugin that serves a model the catalogue does not know offers it there,
and it is selectable in `/model` like any other.
