# HTTP API

Base URL `http://127.0.0.1:8787`. JSON in, JSON out. Errors return **400** with
`{"error": "<redacted, actionable message>"}` — credentials are stripped from
error text before it leaves the process.

There is no authentication layer. FRIDAY is a single-user application and must
stay bound to loopback.

## Health and capability

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/api/health` | Liveness: `{ok, service, version}`. |
| `GET` | `/api/status` | Mode, selected provider and model, the routing reason, whether the hosted credential is present, whether the hosted and local models are reachable, and row counts. |
| `GET` | `/api/tools` | The full catalogue: name, description, permission, timeout, network flag, argument schema, output shape. |

## Configuration

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/api/config` | Effective config, the precedence list, the source of every override, and which keys need a restart. Never includes secrets. |
| `PUT` | `/api/config` | Validate and apply `{"config": {...}}`. Also writes `.friday/config.toml` so the file and the GUI agree. Returns `restart_required`. |
| `POST` | `/api/config/validate` | Dry run: `{valid, error?}`. Applies nothing. |
| `POST` | `/api/config/reset` | **Preview** the defaults alongside the current config. Applies nothing. |

A rejected `PUT` leaves the live configuration untouched.

## Chat and tasks

| Method | Path | Purpose |
| --- | --- | --- |
| `POST` | `/api/chat` | `{message, conversation_id?}` → `{conversation_id, task_id, status, reply, model, iterations, tool_calls, tokens}`. The reply carries its evidence footer. |
| `GET` | `/api/conversations/:id/messages` | Full transcript. |
| `GET` | `/api/tasks?status=&limit=` | Task list with overdue detection. |
| `GET` | `/api/tasks/:id` | Task, its subtasks, its tool-call trace, and the deterministic evidence verdict. |
| `POST` | `/api/tasks/:id/cancel` | Cancel the task and its children. |
| `GET` | `/api/agents?limit=` | Recent agent runs with roles, status, timings and the configured concurrency limits. |

`POST /api/chat` is synchronous — it returns when the run finishes. Follow
progress over the WebSocket.

## Memory

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/api/memories?q=&include_unapproved=` | List or search. Every row carries category, source, provenance and confidence. |
| `POST` | `/api/memories` | Create an approved memory: `{content, category, source, provenance?, tags?}`. |
| `PUT` | `/api/memories/:id` | Edit `{content?, tags?}`. |
| `DELETE` | `/api/memories/:id` | Delete the row and its index entry. |
| `POST` | `/api/memories/:id/approve` | Approve a proposal. |
| `GET` | `/api/memories/conflicts` | Contradictory memories, for the user to resolve. |

Categories: `profile`, `project`, `task`, `knowledge`, `episodic`.
Provenance: `user_stated`, `document`, `task_result`, `model_inference`.

## Research

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/api/research?q=` | Reports plus a `stale` list from the freshness policies. |
| `GET` | `/api/research/:id` | One report with every source URL, publication date and fetch timestamp. |

## Skills

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/api/skills` | The library, the scope meanings and the package format. |
| `POST` | `/api/skills` | `{mode:"template", name, description, scope}` or `{mode:"path", path, scope, enable?}`. |
| `POST` | `/api/skills/inspect` | `{path}` → declared tools split into allowed / blocked / unknown under current policy, plus scripts found. |
| `POST` | `/api/skills/rescan` | Discover hand-copied packages; reports what was rejected and why. |
| `PUT` | `/api/skills/:name/enabled` | `{enabled: bool}`. |
| `POST` | `/api/skills/:name/run` | `{input?}` → `{status, output, task_id}`. |
| `GET` | `/api/skills/:name/runs` | Run history. |
| `DELETE` | `/api/skills/:name?delete_files=` | Detach; delete files only when asked. |

## Jobs and briefing

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/api/jobs` | Job definitions, run history with failures, and the missed-run recovery report. |
| `POST` | `/api/jobs/:name/run?force=` | Run now. Idempotent per job per local date unless forced. |
| `GET` | `/api/briefing?date=` | The saved briefing for a date, or the latest. |

## Approvals and audit

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/api/approvals` | Requests with action, reason, risk and preview. |
| `POST` | `/api/approvals/:id/decide` | `{decision: "approved" \| "denied"}`. Deciding twice is refused. |
| `GET` | `/api/audit?limit=&category=` | Redacted audit entries. |

## Events — `GET /api/events` (WebSocket)

One JSON object per message, discriminated by `type`:

| `type` | Payload |
| --- | --- |
| `message` | `conversation_id`, `role`, `content` |
| `task_update` | `task_id`, `status`, `title` |
| `agent_update` | `run_id`, `task_id`, `role`, `status`, `detail` |
| `tool_call` | `task_id`, `tool`, `ok`, `summary` |
| `approval_required` | `id`, `action`, `risk`, `preview` |
| `notification` | `category`, `message` |

Subscribe before issuing the request you want to watch. The stream is
broadcast, so a late subscriber misses earlier events; durable state is in
`/api/tasks/:id`.

## Example session

```bash
# What can this instance actually do right now?
curl -s localhost:8787/api/status | jq '{mode, selected_model, hosted_nvidia_model_reachable}'

# Ask something, then inspect the evidence behind the answer
TASK=$(curl -s -X POST localhost:8787/api/chat -H 'content-type: application/json' \
  -d '{"message":"what are my open tasks?"}' | jq -r .task_id)
curl -s localhost:8787/api/tasks/$TASK | jq '{status: .task.status, tools: [.tool_calls[].tool], verified: .evidence.verified}'

# Generate today's briefing and read it back
curl -s -X POST localhost:8787/api/jobs/morning-briefing/run | jq .status
curl -s localhost:8787/api/briefing | jq -r .content
```
