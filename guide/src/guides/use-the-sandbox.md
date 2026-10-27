# Use the sandbox

## Enable it

```json
{
  "tools": {
    "sandbox": {
      "enabled": true,
      "network": false,
      "writable": []
    }
  }
}
```

Or for one run:

```sh
norte-code new --sandbox
```

`--sandbox` overrides `tools.sandbox.enabled`; `--no-sandbox` overrides it the
other way. They conflict with each other.

## Network

The sandbox denies the network by default. Allow it only when the command needs
it:

```sh
norte-code new --sandbox --sandbox-network on
```

`--sandbox-network` requires the sandbox. It cannot be combined with
`--no-sandbox`.

## Extra writable roots

```json
{
  "tools": {
    "sandbox": {
      "enabled": true,
      "writable": ["../shared-cache", "/tmp/build-cache"]
    }
  }
}
```

Relative paths resolve against the workspace. Missing paths are ignored. The
workspace itself is always writable.

## What to expect

- Linux requires `bwrap`; macOS requires `sandbox-exec`.
- A missing binary fails the command. There is no unsandboxed fallback.
- Host files remain readable. The sandbox contains writes, process namespaces,
  IPC/UTS/cgroup namespaces and (unless enabled) the network.
- `/tmp` is a private tmpfs on Linux.
- An approval shows the original command plus a sandbox badge; the human
  approves what the model asked for, not the launcher.

## Check the effective state

`--debug` prints the sandbox and network state. `norte-code telemetry` records
the same state on every request, so a past command can be audited after the
fact.

## Scripts are separate

`skill_script` is not wrapped by the command sandbox. It is disabled by default
and requires `skills.allow_scripts`. If you enable it, treat it as granting
arbitrary code execution under your account.
