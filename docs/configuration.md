# Configuration

Every setting is reachable three ways — the GUI, the CLI, and a plain text file
— and all three resolve to the same effective configuration. You never need to
edit Rust to change FRIDAY's behaviour.

## Precedence

```
defaults  <  user config  <  project config  <  environment  <  CLI flag
```

| Layer | Location |
| --- | --- |
| defaults | compiled in (`config::Config::default()`) |
| user config | `$XDG_CONFIG_HOME/friday/config.toml`, else `~/.config/friday/config.toml` |
| project config | `./.friday/config.toml` |
| environment | `FRIDAY_*`, plus `NEBIUS_*` / `OLLAMA_BASE_URL` |
| CLI | `--bind`, `--mode` |

Both files are *partial*: specify only what you want to change. Everything else
keeps the layer below.

See where each value came from:

```bash
friday config show          # effective TOML + the source of every override
curl localhost:8787/api/config | jq .sources
```

## Validation

Validation is strict and the messages are actionable:

- An **unknown key** is an error, not a silently ignored typo
  (`deny_unknown_fields`).
- A **wrong type** names the file: `.friday/config.toml: invalid configuration`.
- An **out-of-range value** states the range:
  `agents.hard_max_subagents must be <= 5 (spec limit)`.
- `mode` must be `auto | online | offline`; `model.provider` must be
  `nebius | ollama | mock`; `timezone` must be a real IANA name;
  `briefing.at_time` must be `HH:MM`.

**An invalid edit never replaces working settings.** Validation runs on the
merged candidate before it is installed, so a bad file at startup aborts with a
message and a bad `PUT /api/config` returns 400 with the live configuration
untouched. There is a test for each.

```bash
friday config validate      # exit 0 and "configuration is valid", or the first problem
friday config init          # print a complete default file to save and edit
```

## Settings reference

### Top level

| Key | Default | Notes |
| --- | --- | --- |
| `bind` | `127.0.0.1:8787` | Listen address. **Restart required.** Do not expose publicly. |
| `database_url` | `sqlite://./data/friday.db?mode=rwc` | **Restart required.** |
| `timezone` | `UTC` | IANA name. Drives scheduling and all displayed dates. |
| `mode` | `auto` | `auto`, `online` or `offline`. Applied live. |

### `[model]`

| Key | Default | Notes |
| --- | --- | --- |
| `provider` | `nebius` | `nebius`, `ollama` or `mock`. |
| `nebius_base_url` | `https://api.tokenfactory.nebius.com/v1` | Use the `us-central1` host if that is your region. |
| `nebius_model` | `nvidia/nemotron-3-super-120b-a12b` | Any eligible NVIDIA open model. |
| `ollama_base_url` | `http://127.0.0.1:11434` | `/v1` is appended automatically. |
| `ollama_model` | `llama3.1` | Used in offline mode. |
| `max_tokens` | `2048` | Per model turn. |
| `temperature` | `0.2` | Low, because the work is tool-driven. |
| `request_timeout_secs` | `120` | Per provider HTTP request. |

The API **key is never a config value**: it is read from `NEBIUS_API_KEY` in
the environment, so exports and the API response can never leak it.

### `[agents]`

| Key | Default | Notes |
| --- | --- | --- |
| `max_concurrent_subagents` | `3` | Must be between 1 and `hard_max_subagents`. |
| `hard_max_subagents` | `5` | Global semaphore size. Capped at 5. |
| `max_subagent_depth` | `1` | Capped at 1; nested delegation is disabled. |
| `max_iterations` | `12` | Model turns per run. |
| `max_tool_calls` | `24` | Tool executions per run. |
| `run_timeout_secs` | `300` | Wall clock for a top-level run. |
| `subagent_timeout_secs` | `120` | Wall clock per child; also caps a child's request. |
| `tool_timeout_secs` | `60` | Upper bound on any tool's own timeout. |

### `[permissions]`

| Key | Default | Notes |
| --- | --- | --- |
| `allow_local_mutation` | `true` | Writing memories, tasks, files, builds. |
| `allow_network` | `true` | Any tool that leaves the machine. |
| `allow_external_side_effect` | `false` | Actions others can see. |
| `allow_privileged` | `false` | Privileged system access. |
| `command_allowlist` | `["cargo","git","npm"]` | Executables `run_build`/`run_project_tests` may spawn. |
| `max_file_bytes` | `524288` | Per file read. |
| `max_tool_output_bytes` | `65536` | Per tool result, after which it is truncated. |
| `confirm_hosted_uploads` | `false` | Require approval before local content reaches a hosted model. |

### `[[workspaces]]`

Tools can only reach paths under a configured root.

```toml
[[workspaces]]
id = "notes"
path = "/home/you/notes"
writable = false

[[workspaces]]
id = "friday"
path = "/home/you/dev/projects/friday"
writable = false
```

`id` is what tools take as the `workspace` argument. An unknown id produces an
error that lists the ids you do have.

### `[skills]`

| Key | Default |
| --- | --- |
| `user_dir` | `$XDG_CONFIG_HOME/friday/skills` |
| `project_dir` | `.friday/skills` |
| `allow_scripts` | `false` |

### `[briefing]`

| Key | Default | Notes |
| --- | --- | --- |
| `enabled` | `true` | Creates the daily `morning-briefing` job. |
| `at_time` | `07:30` | `HH:MM` in `timezone`. |
| `email_provider` | `synthetic` | `gmail`/`microsoft` are declared but not implemented, and error honestly. |
| `calendar_provider` | `synthetic` | Same. |

### `[research]`

| Key | Default | Notes |
| --- | --- | --- |
| `search_provider` | `tavily` | Needs `TAVILY_API_KEY`, else search refuses rather than inventing results. |
| `max_sources` | `6` | |
| `max_page_bytes` | `200000` | Per fetched page. |

## Environment variables

| Variable | Purpose |
| --- | --- |
| `NEBIUS_API_KEY` | Token Factory credential. Without it the hosted model is unavailable. |
| `NEBIUS_BASE_URL`, `NEBIUS_MODEL` | Override the hosted endpoint and model. |
| `TAVILY_API_KEY` | Enables `search_web` / `research_topic`. |
| `OLLAMA_BASE_URL` | Local inference server. |
| `FRIDAY_BIND`, `FRIDAY_DB`, `FRIDAY_TZ`, `FRIDAY_MODE` | Override the matching settings. |
| `XDG_CONFIG_HOME` | Relocates the user config and skill directories. |
| `RUST_LOG` | e.g. `friday=debug`. |

Copy `.env.example` to `.env` and fill it in; `.env` is gitignored.

## Changing settings at run time

```bash
# read
curl localhost:8787/api/config

# dry run
curl -X POST localhost:8787/api/config/validate -H 'content-type: application/json' \
     -d '{"config": { ... }}'

# apply (also writes .friday/config.toml, so the file and the GUI agree)
curl -X PUT localhost:8787/api/config -H 'content-type: application/json' \
     -d '{"config": { ... }}'
```

The response reports `restart_required`. Only `bind` and `database_url` need
one; everything else — including the model, the mode, permissions and
workspaces — applies live, and the model router is rebuilt immediately.

`POST /api/config/reset` returns the default configuration as a **preview**
without applying it, so a reset is always seen before it happens. Exports and
previews contain no secrets, because no secret is ever a config field.
