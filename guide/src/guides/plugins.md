# Plugins

A plugin is a shared library (`.so` / `.dll` / `.dylib`) that adds things to
Norte Code without changing it: slash commands, a status line field, a
banner line, or providers and models in the catalogue. The
[catalog-plugin](https://github.com/JeanVydes/catalog-plugin) is the
ready-made example: it offers a catalogue so you do not write
`providers.json` and `models.json` by hand.

## What a plugin can do

One trait, five optional surfaces (all default to nothing):

| Surface | What it adds |
| --- | --- |
| `commands()` | Slash commands, parsed and completed like the built-in ones. |
| `run()` | What a plugin command prints, as panel lines. |
| `status()` | One field on the status row under the conversation. |
| `banner()` | One line of wordmark under the built-in one. |
| `catalogue()` | Providers and models the catalogue knows for that run. |

A plugin also receives the host context once, before anything is asked of
it: the current session, its model, provider, title and how much of the
context window it uses. It asks for these when it needs them, because the
session moves (compaction opens another one).

A plugin that fails to load is reported and skipped — the server starts
either way. The host checks the library's `norte_plugin_abi_version`
against its own before loading it; a mismatch is refused with both
numbers named, never loaded on a guess.

## The catalogue plugin, in detail

`catalog-plugin` uses only the `catalogue()` surface. Its providers and
models are merged into the catalogue **for that run**; the files you own
are not written and do not have to exist. That is the whole point: a
plugin that serves a model the catalogue does not know offers it there,
and it is selectable in `/model` like any other.
