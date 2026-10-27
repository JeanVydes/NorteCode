# Workspace and sandbox

## The workspace root

The service takes the process's current directory, canonicalizes it when
possible and treats it as the workspace root. File tools resolve their paths
against that root.

A leading directory names a different one:

```sh
norte-code ~/src/thing
```

It is a positional and not a flag, because it is the one thing a person names
when they name anything, and the whole point is that `norte-code ~/src/thing`
means what every other command line means by a leading path. The process itself
moves, before the configuration is read, so the project config that applies is
the named directory's own — reading this directory's `.norte-code/config.json`
for a session about another project would be the wrong project. A directory that
does not exist is refused with its path, because a typo that silently left the
tools reading a tree nobody named is the failure that is hardest to notice.

The footer names that directory in front of the session line, with the home
abbreviated to `~`, because the root is what a person checks when a tool reads
the wrong tree and the footer is where they are looking. `/debug` prints it in
full.

There is no `/workdir` in the middle of a session. The root is inside the tool
registry and the sandbox declaration, both built per turn, so changing it means
rebuilding them at a turn boundary — and deciding what a resumed session's
directory is before it is worth calling a command.

The policy is stricter than a string prefix check:

- The existing prefix of a path is canonicalized, so a symlink cannot silently
  escape the root.
- A dangling symlink is refused.
- FIFOs, sockets and device nodes are refused before a tool can block on one.
- `..` is resolved before the final containment check.

The root is a policy boundary, not an operating-system jail. A command can still
read host files unless the sandbox is enabled.

## The command sandbox

`tools.sandbox.enabled` turns the `command` executor into a wrapped invocation.
The tool keeps its name, schema and arguments; only the process launch changes.
This keeps approvals, history, the effect journal and telemetry honest.

### Linux

Linux uses `bubblewrap`:

```text
bwrap --die-with-parent --new-session --unshare-all --cap-drop ALL \
  --ro-bind / / --dev /dev --proc /proc --tmpfs /tmp \
  --bind <workspace> <workspace> [--bind <extra> <extra>] \
  [--share-net] --chdir <cwd> -- <original command>
```

The workspace and configured extra paths are writable; the rest of the
filesystem is read-only; `/tmp` is private; the network is denied unless
sandbox networking is enabled.

### macOS

macOS uses `sandbox-exec -p <profile> -- <original command>`. The profile keeps
reads, denies writes outside the workspace, the extra writable roots and the
temporary roots, and denies network access unless enabled.

### Failure is closed

A missing sandbox binary is an error. There is no unsandboxed fallback. The
sandbox is not a virtual machine: the host filesystem remains readable, Unix
sockets visible in the filesystem may still be reachable, and CPU, memory, disk
and the kernel are shared.

## Flags

`--sandbox`, `--no-sandbox` and `--sandbox-network on|off` override the config
for one run or resume. `--sandbox-network` requires the sandbox. `--no-sandbox`
cannot be combined with either.

```sh
norte-code new --sandbox
norte-code new --sandbox --sandbox-network on
norte-code new --no-sandbox
```

## Extra writable roots

```json
{
  "tools": {
    "sandbox": {
      "enabled": true,
      "network": false,
      "writable": ["../shared-cache"]
    }
  }
}
```

Relative paths resolve against the workspace; missing paths are ignored. Keep
the list small: every extra root is a deliberate hole in the write boundary.

## What the sandbox does not cover

`skill_script` runs outside the command sandbox. It is disabled by default and
requires `skills.allow_scripts`. Treat enabling it as granting arbitrary code
execution under the user's account.

The sandbox also does not protect secrets you put in files readable by the
workspace. API keys are redacted from command output registered with
`with_secrets`, but a command that reads a key from an unprotected file can
still print it.
