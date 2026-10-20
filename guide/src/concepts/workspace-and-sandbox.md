# Workspace and sandbox

## The workspace root

The service takes the process's current directory, canonicalizes it when
possible and treats it as the workspace root. File tools resolve their paths
against that root.

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
norte-code run --sandbox "run the test suite"
norte-code run --sandbox --sandbox-network on "fetch the dependency"
norte-code run --no-sandbox "inspect the host toolchain"
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
