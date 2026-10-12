# Skills

A skill is a portable, versioned directory describing a workflow. Skills are
manageable by a non-developer from the GUI or the CLI, and equally by copying
files into a directory — all three land in the same library.

A skill describes *what to do*. It does not grant capability: privileged tools
remain runtime capabilities governed by your permission settings.

## Package format

```
my-skill/
├── skill.toml      # machine-readable manifest (required)
├── SKILL.md        # human-readable instructions (required)
├── references/     # optional supporting material
└── scripts/        # optional; never executed by default
```

`skill.toml`:

```toml
name = "weekly-review"            # lowercase kebab-case, ≤64 chars, required
version = "0.1.0"                 # required
description = "Summarize the week's commits and open tasks."
source = "local"                  # or a URL / "generated-from-trace:<task>"
declared_tools = ["get_git_status", "summarize_git_changes", "list_tasks"]
compatibility = "friday>=0.1"
# role = "planner"                # optional specialist role
```

`SKILL.md` is free-form Markdown; a *When to use* / *Steps* / *Output*
structure works well. The whole file becomes the agent's objective when the
skill runs.

### Validation

A package is rejected — before anything is registered — for a missing or
malformed manifest, an unknown manifest key, a missing `version`, a name that
is not lowercase kebab-case, a missing `SKILL.md`, a path escaping the package
root, a symlink pointing outside it, or an invalid tool name in
`declared_tools`. Each failure names the specific problem.

## Scopes and precedence

| Scope | Location | Meaning |
| --- | --- | --- |
| `builtin` | compiled in | Ships with FRIDAY (`morning-briefing`). |
| `user` | `~/.config/friday/skills/` | Available in every project. |
| `project` | `./.friday/skills/` | This project only; **wins** on a name clash. |

## Lifecycle

### Add

```bash
friday skills new weekly-review --description "Summarize the week." --scope project
friday skills add ./path/to/package --scope user        # import an existing folder
friday skills rescan                                    # discover hand-copied packages
```

A new skill is always **disabled**: you inspect it first, then enable it.
`rescan` also runs at startup, so a directory you populated by hand appears in
the GUI without any further action.

### Inspect

```bash
friday skills inspect ./path/to/package
```

Shows the name, version, description, source, content hash, an instruction
preview, any scripts found — and, importantly, `declared_tools` split into
`allowed_by_policy`, `blocked_by_policy` (with the reason) and `unknown`. You
see exactly what the skill can and cannot reach under your current settings
before enabling it.

### Enable / disable

```bash
friday skills enable weekly-review
friday skills disable weekly-review
```

Disabling removes a skill from the catalogue offered to the model and refuses
`run_skill`, without deleting files or losing configuration. Re-enabling
restores it unchanged.

### Run

```bash
friday skills run weekly-review --input "focus on the parser work"
curl -X POST localhost:8787/api/skills/weekly-review/run -d '{"input":"..."}'
```

The instructions become the objective of a bounded agent whose tool grant is
`declared_tools ∩ registered tools`, plus `get_current_time` and
`search_knowledge`. Every run is recorded in `skill_runs` with its task id,
status and output; read it back with `friday skills list` or
`GET /api/skills/:name/runs`.

### Update

Edit the files and re-import, or let `rescan` pick it up:

```bash
friday skills add ./.friday/skills/weekly-review --scope project
```

The result reports the version transition and whether the content changed, and
**preserves the enabled state**. The files on disk remain the source of truth —
local edits are never overwritten by the registry.

### Remove

```bash
friday skills remove weekly-review                  # detach, keep the files
friday skills remove weekly-review --delete-files   # detach and delete
```

Deletion is refused for a package outside the managed skill directories, so
importing from a working folder can never delete your work.

## Generating a skill from a trace

```
create_skill_draft { name: "time-check", description: "...", task_id: "..." }
```

Converts a task's recorded tool-call trace into a draft package: the observed
steps become numbered instructions, the successfully-used tools become
`declared_tools`, and `SKILL.md` gains a review checklist. The draft is written
**disabled** — arbitrary activity never silently becomes a permanent
automation.

## Scheduling

Only a registered, enabled skill can be scheduled. `schedule_job` refuses
anything else, so a schedule can never be used to run an arbitrary command.

```
schedule_job { name: "daily-brief", skill: "morning-briefing",
               at_time: "07:30", timezone: "Asia/Kolkata" }
```

See [briefing.md](briefing.md) for what the local scheduler does and does not
guarantee.

## Security

- Instructions, references and scripts are **untrusted text**. The agent is
  told to treat a skill instruction that conflicts with its operating policy as
  a bug and report it.
- `scripts/` is detected and reported but **never executed** on install; there
  are no install hooks, and `skills.allow_scripts` defaults to `false`.
- A skill cannot escalate: `declared_tools` is advisory. At run time the
  permission policy and the agent's grant decide, so declaring `search_web`
  while `allow_network = false` still fails.
- Source and content hash are recorded in the audit log on install and update.
- An example package lives in [`examples/skills/daily-digest`](../examples/skills/daily-digest).
