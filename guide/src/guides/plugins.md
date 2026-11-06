# Plugins

A plugin is a shared library (`.so` / `.dll` / `.dylib`) that adds things to
Norte Code without changing it: slash commands, a status line field, a
banner line, or providers and models in the catalogue. The
[catalog-plugin](https://github.com/JeanVydes/catalog-plugin) is the
ready-made example: it offers a catalogue so you do not write
`providers.json` and `models.json` by hand.

## Install the OpenRouter catalogue

```sh
norte-code plugins add https://github.com/JeanVydes/catalog-plugin --package catalog-plugin-openrouter
```

For a workspace with several plugin packages, `--all` installs every one,
one after another; a failure in one does not stop the rest:

```sh
norte-code plugins add https://example.com/repo --all
```

Each install reports its steps as it goes — `cloning`, `inspecting the
workspace`, `building <package>`, `installing` — with a counter (`[2/4]`),
without dumping the build log. Only a failure prints why.

This clones the repository, builds the plugin and installs the library into
`~/.config/norte-code/plugins/` (or `$NORTE_CODE_PLUGINS`). The same command
exists in the chat as `/plugins add URL [--package NAME | --all]`.

> **Trust first.** Installing a plugin clones and builds untrusted
> repository code — including its `build.rs` — before loading the resulting
> library. That is arbitrary code execution by design. Only install from
> sources you trust, and review a plugin's code before adding it.

Without `--package` and without `--all`, the install **does not guess**: if
the repository is a workspace with exactly one plugin package, that one is
selected automatically; if it has several, the install fails and lists them
— you pick one with `--package` or all of them with `--all`.

```sh
norte-code plugins add https://example.com/repo --package my-plugin
```

A plugin has to expose exactly one `cdylib` target with one plugin in it.
A package with several `cdylib` targets is refused with their names.

## Manage them

```sh
norte-code plugins list            # installed, enabled or not, and why not
norte-code plugins update [NAME]   # rebuild one, or every one with a recorded source
norte-code plugins enable NAME     # load it again at the next service start
norte-code plugins disable NAME    # skip it at the next service start
norte-code plugins rm NAME         # remove the library and its recorded source
```

The same commands exist in the chat under `/plugins …`. The server owns plugin
loading and the catalogue: changes take effect after the service restarts
(or its catalogue is refreshed), not when the stateless CLI reconnects.

`enable` and `disable` rename the library between `.so` and
`.so.disabled`. The loader only opens `.so`/`.dll`/`.dylib`, so a disabled
plugin is silently skipped.

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
and it is selectable in `norte-code models` and `/model` like any other.
