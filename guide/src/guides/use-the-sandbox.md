# Use the sandbox

By default (`tools.confinement.sandbox.enabled: false`) the shell tool is
only an argument gate: it may reach outside the workspace. Enable the
sandbox to contain writes, process namespaces and the network.

## Enable it

```json
{
  "tools": {
    "confinement": {
      "sandbox": { "enabled": true, "network": false, "writable": [] }
    }
  }
}
```

Or for one run:

```sh
norte-code new --sandbox
```

`--sandbox` overrides the config one way, `--no-sandbox` the other. They
conflict with each other.

## Network

The sandbox denies the network by default. Allow it only when the command
needs it:

```sh
norte-code new --sandbox --sandbox-network on
```

`--sandbox-network` requires the sandbox.

## Extra writable roots

```json
{
  "tools": {
    "confinement": {
      "sandbox": { "enabled": true, "writable": ["../shared-cache"] }
    }
  }
}
```

Relative paths resolve against the workspace; missing paths are ignored.
The workspace itself is always writable. Keep the list small: every extra
root is a deliberate hole in the write boundary.

## What to expect

- Linux requires `bwrap`; macOS requires `sandbox-exec`. A missing binary
  fails the command — there is no unsandboxed fallback.
- Host files stay readable. The sandbox contains writes, namespaces and
  (unless enabled) the network; it is not a virtual machine.
- `/tmp` is private on Linux.
- Approval rules see the command the model asked for, not the launcher.

## Check the effective state

`--debug` prints the sandbox and network state, and `norte-code telemetry`
records it on every request, so a past command can be audited after the fact.

## Scripts are separate

`skill_script` is not wrapped by the command sandbox. It is disabled by
default and requires `skills.allow_scripts`. Enabling it grants arbitrary
code execution under your account — treat it as a trust decision.
