# FRIDAY — Personal AI Operating System

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

A persistent personal assistant that remembers, schedules, researches with
citations, coordinates bounded specialist agents, and tells you the truth about
what it actually did.

Built in Rust for the **Nebius × NVIDIA Global AI Hackathon 2026** (Personal AI),
using the hosted **NVIDIA Nemotron** model through **Nebius Token Factory**.

> **Status:** the backend is implemented and tested — 110 automated tests over
> the real database, tool registry, agent loop and HTTP server. See
> [What is verified](#what-is-verified) for an honest line between tested
> functionality and work in progress.

## Why

Chat assistants start from zero every time. You re-explain your project, you
re-find the article you read last week, and when the assistant says "done" you
have no way to check. FRIDAY is built around the opposite defaults:

- **Continuity.** Memory, tasks, research and decisions persist across
  restarts, with provenance on every record.
- **Evidence.** A task is complete when recorded tool results say so — not when
  a model sounds confident. Every answer carries the tools it used.
- **Honesty.** When something can't be done, it says exactly why. It will not
  fabricate a citation, a date, a test result, or a completed action.
- **Control.** Configuration, permissions and skills are yours to edit, from
  the GUI, the CLI, or a text file. No source changes required.

## What makes it different

Persistent agents, editable memory, schedules and skills all exist elsewhere
(Letta, OpenClaw and others ship versions of each). FRIDAY does not claim those
concepts are new. Two things are implemented here in a way that is meant to be
measurably different:

### 1. Proof-Carrying Actions

Completion is a deterministic check, not a model's opinion. `evidence::verify`
reads the `tool_calls` and `evidence` tables and refuses a claim that asserts
an action with no successful tool call behind it:

```
> update the files
All done, I updated the files!

---
Evidence (0 tool calls)
```
```jsonc
// verify_task_completion
{ "verified": false,
  "missing": ["no tool call succeeded for this task, so nothing has been verified",
              "the claim asserts an action was performed, but no successful tool call exists"],
  "instruction": "Do NOT report this task as complete." }
```

Failures are disclosed too — a failed tool shows up in the footer as
`read_local_file (failed)` rather than quietly vanishing.

### 2. Online-to-Offline knowledge handoff

Research is saved with its question, every source URL, publication dates and
fetch timestamps, under a per-topic freshness policy. `prepare_offline_packet`
bundles a topic into one self-contained document. Pull the network cable and
the library still answers — while disclosing how old it is:

```bash
# online
research_topic → save_research_report

# offline: search_web is refused with a pointer to the local alternative;
# search_knowledge answers, and every hit reports age_days and stale
```

## Quick start

### Requirements

- Rust stable (1.80+; developed on 1.97)
- Node 20+ (only for the frontend)
- A [Nebius Token Factory](https://dev.nebius.com/builders) API key for the
  hosted NVIDIA model
- Optional: [Ollama](https://ollama.com) for offline inference,
  [Tavily](https://tavily.com) for web search

### Run it

```bash
git clone <this repo> && cd friday
cp .env.example .env         # then add NEBIUS_API_KEY
cargo run                    # http://127.0.0.1:8787
```

```bash
curl -s localhost:8787/api/status | jq '{mode, selected_model, hosted_nvidia_model_reachable}'
```

It starts without a key — it will tell you the hosted model is unavailable
rather than failing obscurely — but the hackathon demo path needs the key.

### Point it at your files

```toml
# .friday/config.toml
timezone = "Asia/Kolkata"

[[workspaces]]
id = "notes"
path = "/home/you/notes"
```

Tools can only reach configured workspace roots. Run `friday config init` for a
complete annotated default file.

### Try it from the CLI

```bash
cargo run -- ask "what can you do offline right now?"
cargo run -- brief                          # today's briefing from demo data
cargo run -- skills add examples/skills/daily-digest --scope project
cargo run -- skills inspect examples/skills/daily-digest
cargo run -- tools                          # the whole catalogue
cargo run -- config show                    # effective settings + their source
```

### Tests

```bash
cargo test            # 110 tests, no network required
```

## How it works

```
React UI ─┐
          ├─ HTTP/WS ─► axum API ─► agents ─► tools (permission layer) ─► SQLite
friday CLI┘                           │                  │
                                      │                  └─► files · git · web
                                      └─► model router ─► Nebius Token Factory
                                                          (NVIDIA Nemotron)
                                                        · Ollama · mock
```

One binary, one configuration model, one tool registry — so the GUI and the CLI
can never disagree. [Architecture →](docs/architecture.md)

### The agent loop

Control flow is deterministic Rust; the model only proposes. Each turn: build
the prompt (role, policy, real clock, retrieved memories labelled as data,
enabled skills) → offer only the tools the caller may actually use → execute
any requested tool through the single permission-checked path → repeat until an
answer or a budget boundary. Every exit is explicit: `completed`, `failed` or
`cancelled`, and it says which.

### Bounded multi-agent orchestration

A parent can fan out to 1–5 specialists (planner, researcher, summarizer,
coder, verifier, memory, briefing, offline), 3 concurrent by default. The
guarantees are enforced in code, not prompted:

- A child's tool grant is `requested ∩ role default ∩ parent grant`, minus a
  denylist. **It can never widen its own permissions.**
- Depth is capped at 1 — no recursive spawning.
- Concurrency is bounded twice: a global semaphore plus a per-fan-out limiter.
- Cancellation propagates from parent to every child.
- The parent must merge results and report child failures.

### Security

Enforced outside the model, because a prompt is not an access-control
mechanism: untrusted content is wrapped and labelled; commands run as argument
arrays against an allowlist with no shell; paths are confined to workspace
roots with traversal, symlink and secret-file refusal; URLs are validated
against private ranges before *and after* redirects; secrets are redacted
before anything is logged; irreversible actions need a recorded human approval.
[Security →](docs/security.md)

## Features

| | |
| --- | --- |
| **Memory** | 5 categories, provenance + confidence, FTS search, propose-vs-save consent, dedup, conflict detection, export, deletion that clears the index |
| **Agents** | Bounded loop, 8 specialist roles, budgets, cancellation, restart recovery, cost estimates vs actuals |
| **Tools** | 69 implemented tools, 5 permission categories, per-call timeouts, output caps, full audit trail |
| **Research** | Cited reports, freshness policies, offline packets, claim reconciliation, knowledge gaps |
| **Briefing** | Mail + calendar + tasks + deadlines, source links, injection warnings, per-date idempotency |
| **Skills** | Portable `skill.toml` + `SKILL.md`, GUI/CLI/file-managed, inspect before enable, trace-to-draft generation |
| **Offline** | Capability reports, network-tool filtering, local model routing, honest refusals |
| **Config** | Documented precedence, strict validation, source reporting, live reload |

[Full tool catalogue →](docs/tools.md)

## Hackathon technology

| Requirement | How FRIDAY satisfies it |
| --- | --- |
| **NVIDIA open model** | `nvidia/nemotron-3-super-120b-a12b` (configurable) |
| **Nebius** | Nebius Token Factory, OpenAI-compatible `/chat/completions` with tool calling, at `https://api.tokenfactory.nebius.com/v1` |
| **Meaningful use** | The model drives the agent loop: it plans, selects and parameterises tools from 69 schemas, synthesizes briefings and cited research reports, and acts as the specialist in every subagent role. It is not decoration — remove it and the application does nothing. |
| **Runtime** | `models::OpenAiCompatible` with bearer auth, retry with backoff on 429/5xx, timeouts, token-usage accounting, and per-request latency recorded in `agent_runs` |

The hosted model is required for the demo path. A deterministic mock provider
exists for tests only and identifies itself as `mock` — it is never presented
as the NVIDIA model.

## What is verified

**Tested by the automated suite** (110 tests, no network):

- Memory survives a restart, is found by search, and reaches the model labelled
  as data; unapproved proposals are excluded until approved; deletion clears
  the index.
- The agent executes real tools and uses their results; budgets, cancellation
  and restart recovery behave; a provider error fails honestly.
- Three specialists run independently with structured results; grants intersect
  in both directions; cancellation propagates; the hard maximum is enforced.
- All six skill-lifecycle criteria, including malformed-package rejection,
  symlink-escape refusal, and scripts detected but never executed.
- The briefing is reproducible and idempotent per date, with source links,
  flagged relative dates, and the injection attempt surfaced.
- Research keeps its sources and answers with the network off; unconfigured
  search refuses instead of inventing results.
- The full config precedence chain; a bad edit never replaces working settings.
- The security boundary: traversal, SSRF, secret files, permission denial,
  approval gating, redaction, output caps, command allowlisting.

**Implemented, exercised manually, not yet covered end to end:**

- Live calls against Nebius Token Factory (the suite uses the mock by design;
  `cargo run -- ask` with a real key is the manual check).
- Tavily-backed `search_web` / `research_topic` against the live API.
- Ollama offline inference.

**Not implemented** — and the code says so at run time rather than pretending:

- Gmail / Microsoft Graph connectors (`briefing.email_provider` refuses them).
- Voice (`transcribe_audio`, `speak_response`) and screen understanding
  (`capture_screen`, `analyze_screen`).
- The React frontend is in progress; the API and CLI are complete.

## Documentation

- [Architecture](docs/architecture.md) — modules, agent loop, orchestration, persistence
- [Security & privacy](docs/security.md) — threat model and the control for each entry
- [Configuration](docs/configuration.md) — precedence, validation, every setting
- [Skills](docs/skills.md) — package format and lifecycle
- [Briefing & scheduling](docs/briefing.md) — connectors, deadlines, idempotency
- [HTTP API](docs/api.md) — endpoints and WebSocket events
- [Tool catalogue](docs/tools.md) — generated from the live registry

## Limitations

- Single user, no authentication. Keep it on loopback.
- The scheduler is local: it cannot fire while the machine is asleep.
- Retrieval is FTS5 + LIKE, not embeddings. Fine at this corpus size.
- Subagents cannot nest. This is a safety property, not a gap.
- SQLite only.

## License

MIT — see [LICENSE](LICENSE).
