# Architecture

FRIDAY is one Rust binary. It serves an HTTP + WebSocket API that the React
frontend and the CLI both consume, so there is exactly one implementation of
configuration, permissions and the tool catalogue — the GUI and the command
line cannot disagree.

```
                 ┌──────────────┐     ┌──────────────┐
                 │  React UI    │     │  friday CLI  │
                 └──────┬───────┘     └──────┬───────┘
                        │  HTTP / WS         │  in-process
                        └─────────┬──────────┘
                                  ▼
                        ┌───────────────────┐
                        │   api (axum)      │
                        └─────────┬─────────┘
                                  ▼
          ┌───────────────────────────────────────────────┐
          │                 agents                        │
          │  bounded loop · parent/child orchestration    │
          └───┬───────────────┬───────────────────┬───────┘
              ▼               ▼                   ▼
      ┌──────────────┐ ┌──────────────┐  ┌─────────────────┐
      │ tools        │ │ models       │  │ memory/evidence │
      │ (registry +  │ │ (provider    │  │ research/skills │
      │  permissions)│ │  router)     │  │ briefing/sched. │
      └──────┬───────┘ └──────┬───────┘  └────────┬────────┘
             │                │                   │
             ▼                ▼                   ▼
        ┌─────────┐   ┌────────────────┐    ┌──────────┐
        │ SQLite  │   │ Nebius Token   │    │  local   │
        │  (WAL)  │   │ Factory /      │    │ files &  │
        └─────────┘   │ Ollama / mock  │    │  git     │
                      └────────────────┘    └──────────┘
```

## Module responsibilities

| Module | Responsibility |
| --- | --- |
| `config` | One typed model of every setting, with precedence, validation and source reporting. |
| `db` | Connection pooling and tracked, idempotent migrations. |
| `util` | The primitives security depends on: redaction, path resolution, URL validation, untrusted-content wrapping. |
| `models` | `ModelProvider` trait, the Nebius/Ollama OpenAI-compatible client, the mock, and connectivity-aware routing. |
| `state` | Shared application state: pool, live config, router, registry, subagent semaphore, cancellation map, event bus. |
| `tools` | The tool registry and the single permission-checked execution path. |
| `memory` | Persistent memory with categories, provenance, search and conflict detection. |
| `evidence` | Proof-Carrying Actions: deterministic completion verification. |
| `research` | Search, extraction, cited reports, freshness, offline packets. |
| `agents` | The bounded agent loop and the subagent orchestrator. |
| `skills` | Skill package format, validation and lifecycle. |
| `briefing` | Mail/calendar connectors and the deterministic briefing inputs. |
| `scheduler` | Timezone-aware local scheduling with per-date idempotency and recovery. |
| `api` | The HTTP/WebSocket surface. |

## The agent loop

`agents::run` is the only place a model is consulted, and it is deliberately
boring. Control flow is Rust; the model only proposes.

1. Build the system prompt: role instructions, the operating policy, the
   current time and timezone, retrieved memories (labelled as data) and the
   enabled-skill catalogue.
2. Offer the tool schemas the caller is actually allowed to use — the
   subagent's grant intersected with what offline mode permits.
3. Ask the provider for one turn.
4. If it returned no tool calls, that text is the answer; stop.
5. Otherwise execute each requested tool through `tools::execute`, appending
   each structured result as a `tool` message.
6. Repeat until an answer, cancellation, or a budget boundary.

Every exit is explicit and recorded: `completed`, `failed`, `cancelled`. The
loop stops on `agents.max_iterations`, `agents.max_tool_calls`, the per-run
deadline, a provider error, or a cancellation token — and says which.

### Why control flow is not delegated to the model

A model that decides its own stopping condition can loop forever, and a model
asked "did that work?" will often say yes. Budgets, cancellation, result
merging and completion verification are therefore all deterministic Rust.

## Multi-agent orchestration

A parent task may fan out to between one and five specialist subagents.

- **Roles** (`agents::ROLES`) each carry a prompt and a default tool grant:
  planner, researcher, summarizer, coder, verifier, memory, briefing, offline.
- **Concurrency** is bounded twice: a global `Semaphore` sized to
  `agents.hard_max_subagents`, and a per-fan-out limiter sized to
  `agents.max_concurrent_subagents` (default 3).
- **Depth** is capped at 1. `spawn_subagent` is refused when `ctx.depth > 0`,
  so there is no recursive spawning.
- **Grants** are intersections: `requested ∩ role default ∩ parent grant`,
  minus a hard denylist (`spawn_subagent`, `cancel_task`, `delete_memory`,
  `save_memory`, `schedule_job`). A child can never widen its permissions.
- **State** lives in the database. Each child is a `tasks` row parented to the
  caller plus an `agent_runs` row, so progress survives a restart and the UI
  reads it from one place.
- **Cancellation** propagates: children clone the parent's `CancellationToken`.
- **Merging** is the parent's job. `spawn_subagent` returns every child's
  status, including failures, and instructs the parent not to drop them.

Parallelism is only used where subtasks are genuinely independent — the tool
description says so, and `detect_duplicate_work` exists to catch overlap.

## Persistence

SQLite in WAL mode. Migrations are plain SQL, split on statement boundaries,
applied in a transaction and recorded in `_migrations`, so startup is
idempotent. An in-memory URL automatically drops the pool to one connection,
since an in-memory database is private per connection.

Key tables: `conversations`/`messages`, `memories` (+ `memories_fts`),
`tasks`/`task_events`, `agent_runs`, `tool_calls`, `evidence`, `skills`/
`skill_runs`, `scheduled_jobs`/`job_runs`, `research_documents`/
`research_sources` (+ `research_fts`), `approval_requests`, `inbox`,
`calendar_events`, `briefings`, `audit_log`.

## Model routing

`models::Router` holds up to three providers and picks one per request mode:

| Mode | Behaviour |
| --- | --- |
| `online` | Hosted NVIDIA Nemotron via Nebius Token Factory. Errors if no credential. |
| `offline` | Local model only. Errors — with instructions — if none is reachable. |
| `auto` | Hosted if credentialed, else local, else the mock. |

Offline mode never silently falls back to a hosted provider, and network tools
are filtered out of the schema list entirely rather than failing mid-call.

## Data flow for a typical request

`POST /api/chat` → create conversation/task rows → load recent history and
relevant memories → `agents::run` → tool calls through the permission layer →
persist the assistant message → append the evidence footer → broadcast
`TaskUpdate`/`ToolCall`/`Message` events over the WebSocket.

## Deliberate non-goals

- No Python in the core. Nothing in the MVP needed it.
- No microservices. Subagents are Tokio tasks in one process; the database is
  the coordination point.
- No vector store yet. FTS5 with a LIKE fallback is enough at this corpus
  size, and embeddings can be added behind `memory::search` without touching
  callers.
- No nested delegation. Depth 1 is a safety property, not a limitation to work
  around.
