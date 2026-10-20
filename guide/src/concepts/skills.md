# Agent Skills

## What a skill is

A skill is a directory with a `SKILL.md` file: YAML frontmatter plus Markdown
instructions, optionally with `scripts/`, `references/` and `assets/`. Norte Code
follows the [Agent Skills specification](https://agentskills.io/specification).

The frontmatter carries at least:

```yaml
---
name: release-checklist
description: Check a release branch before tagging.
---
```

`name` must match the directory name. `description` is what the model sees when
deciding whether to load the skill.

## Discovery roots

Skills are discovered from these roots, in increasing precedence:

1. Bundled examples next to the executable (`assets/skills`), when present.
2. The user root, normally `$XDG_CONFIG_HOME/norte-code/skills` or
   `~/.config/norte-code/skills`.
3. The interop user root `~/.agents/skills`, when `skills.interop` is true.
4. Each path in `skills.paths`, relative to the workspace unless absolute.
5. `.norte-code/skills` in the workspace.
6. `.agents/skills` in the workspace, when `skills.interop` is true.

A later root shadows an earlier one with the same name, and the shadowing is
reported. A malformed skill is skipped with a warning; discovery never fails
and never executes anything.

## Progressive disclosure

Three tools keep skill content out of the prompt until it is needed:

| Tool | Purpose |
| --- | --- |
| `skill` | List skills, or activate one and return its instructions. |
| `skill_resource` | Read a file under `references/`, `scripts/` or `assets/`. |
| `skill_script` | Run a bundled script, only when scripts are enabled. |

The `skill` body is capped at 64 KiB, resources at 256 KiB by default, and
script output at 32 KiB per stream before it reaches the model.

## Configuration

```json
{
  "skills": {
    "enabled": true,
    "paths": ["../shared-skills"],
    "interop": true,
    "allow_scripts": false,
    "honor_allowed_tools": false,
    "max_skills": 128
  }
}
```

- `enabled` turns discovery and the skill tools off when false.
- `allow_scripts` registers `skill_script`. It is off by default because scripts
  run arbitrary code and are **not** covered by the command sandbox.
- `honor_allowed_tools` lets a skill's experimental `allowed-tools` field
  pre-approve calls. Leave it off unless you trust the skill source.
- `max_skills` bounds memory and prompt exposure.

`/refresh` in the interactive prompt re-scans the roots; the next turn registers
the refreshed tools. `norte-code skills check` validates every skill and prints
the warnings.

## What discovery does not do

It does not install skills, resolve dependencies or run scripts during
discovery. A script is only a candidate until the model calls `skill_script`
and the host allows it.
