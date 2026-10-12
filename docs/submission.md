# Devpost submission

Draft copy for the submission form. Track: **Personal AI**.
Requirements checked against the live Devpost page on 12 Oct 2026 — see
[hackathon.md](hackathon.md).

---

## Tagline

A personal AI operating system that remembers across restarts, works offline,
and proves what it actually did.

## What I built

FRIDAY is a personal AI operating system: a single Rust service that gives
NVIDIA Nemotron — served through Nebius Token Factory — persistent memory, 75
permission-checked tools, reusable skills, scheduled workflows, and bounded
multi-agent orchestration. It has a React UI and a CLI with full parity, and it
runs entirely on your own machine except for the model call itself.

It does the ordinary personal-assistant things: a morning briefing from your
real (read-only) Gmail and calendar, research with real citations, reminders and
deadlines, repository inspection, voice and screen. What makes it different is
how it behaves when things are uncertain.

## Why I built it

Three failures recur with every assistant I use.

**It forgets.** Every session starts by re-explaining the project.

**It cannot be checked.** It says "done" and there is no way to tell whether
anything happened. This is the one that actually costs you — a confidently
wrong "I updated the files" is worse than an error.

**It is useless without a connection**, even for questions it already has the
answer to, saved locally, from last week.

FRIDAY inverts all three defaults: memory is a durable row with provenance;
completion is a database check over recorded tool results; and saved research
stays answerable with citations and an age when the network is gone.

## How it works

```
React UI ─┐
          ├─ HTTP/WS ─► axum API ─► agents ─► tools (permission layer) ─► SQLite
friday CLI┘                           │                  │
                                      │                  └─► files · git · web · screen · mic
                                      └─► model router ─► Nebius Token Factory
                                                          (NVIDIA Nemotron)
                                                        · Ollama · mock
```

One binary, one configuration model, one tool registry — so the GUI and the CLI
can never disagree about what is permitted.

**The agent loop is deterministic Rust; the model only proposes.** Each turn
FRIDAY builds the prompt (role, policy, the real system clock, retrieved
memories labelled as data, enabled skills), offers only the tools the caller
may actually use, executes any requested tool through a single
permission-checked path, and repeats until an answer or a budget boundary. Every
exit is explicit: `completed`, `failed`, or `cancelled`, and it says which. A
model that picks its own stopping condition loops forever, and a model asked
"did that work?" says yes — so budgets, cancellation, result merging and
completion verification are all code.

**Multi-agent work is bounded by construction.** A parent fans out to 1–5
specialists (planner, researcher, summarizer, coder, verifier, memory, briefing,
offline), 3 concurrent by default. A child's tool grant is the intersection of
what it requested, its role's default, and its parent's grant, minus a
denylist — it can never widen its own permissions. Depth is capped at 1, so
nothing recurses. Cancellation propagates to every child.

**Security is enforced outside the model**, because a prompt is not an
access-control mechanism. Retrieved content — mail, web pages, files,
screenshots, transcripts — reaches the model only inside an
`<untrusted_content>` block that tells it to report instructions rather than
follow them. There is no shell: commands run as argument arrays against an
allowlist. Paths are confined to configured workspace roots with traversal,
symlink and secret-file refusal. URLs are validated against private ranges
before *and after* redirects. Secrets are redacted before anything is logged.
Irreversible actions need a recorded human approval, and a *pending* approval is
not an approval.

## The two differentiators

Persistent agents, editable memory, schedules and skills all exist elsewhere.
I am not claiming those concepts are new. Two things here are implemented in a
way meant to be measurably different.

### Proof-Carrying Actions

Completion is a deterministic check, not a model's opinion. `evidence::verify`
reads the recorded `tool_calls` and refuses a claim that asserts an action with
no successful tool call behind it:

```
> update the files
All done, I updated the files!

---
Evidence (0 tool calls)
```
```jsonc
{ "verified": false,
  "missing": ["no tool call succeeded for this task, so nothing has been verified",
              "the claim asserts an action was performed, but no successful tool call exists"],
  "instruction": "Do NOT report this task as complete." }
```

Failures are disclosed too: a failed tool shows in the footer as
`read_local_file (failed)` rather than quietly vanishing. A research report
cannot be saved without at least one valid public source URL.

### Online-to-offline knowledge handoff

Research is saved with its question, every source URL, publication dates and
fetch timestamps, under a per-topic freshness policy.
`prepare_offline_packet` bundles a topic into one self-contained document. Pull
the network and the library still answers — while disclosing how old it is.
Live `search_web` refuses, names the reason, and points at the local
alternative. Offline is a routing decision, not degraded mode.

## Nebius and NVIDIA

| | |
| --- | --- |
| **Model** | `nvidia/nemotron-3-super-120b-a12b`, configurable |
| **Infrastructure** | Nebius Token Factory, OpenAI-compatible `/chat/completions` with tool calling, at `https://api.tokenfactory.nebius.com/v1` |
| **Meaningful use** | The model *is* the control system: it plans, selects and parameterises tools from 75 JSON schemas, synthesizes briefings and cited research reports, and acts as the specialist in all eight subagent roles. Remove it and the application does nothing. |
| **Model tiering** | Per-role selection, as the track encourages: a `reasoning_model` for the generalist, planner, coder and verifier; a `fast_model` for the summarizer, researcher, memory, briefing and offline roles, which make many shallow calls. `friday status` shows the model each role would use, and `agent_runs` records which one actually ran. |
| **Client** | Bearer auth, retry with backoff on 429/5xx, timeouts, token accounting, per-request latency recorded per agent run. Multimodal content parts for vision, and `/audio/transcriptions` for hosted speech-to-text. |
| **Tavily** | Search provider for the research pipeline. Every claim is attributable to a URL, fetch timestamps are stored, fabricated and private-address citations are rejected, and the result stays usable offline. |

A deterministic mock provider exists for tests and identifies itself as
`mock` — it is never presented as the NVIDIA model.

## What is verified, and what is not

**144 automated tests**, no network required, over the real database, tool
registry, agent loop and HTTP server — not a parallel harness. They cover
memory surviving a restart, budget and cancellation behaviour, grant
intersection in both directions, all six skill-lifecycle criteria, briefing
idempotency and injection reporting, offline routing, the full configuration
precedence chain, and the security boundary (traversal, SSRF, secret files,
permission denial, approval gating, redaction, output caps, command
allowlisting).

**Verified against live APIs:** the Tavily research pipeline end to end (4
search results, 3 of 3 sources fetched, report persisted with 3 citations) and
screen capture producing a real PNG.

**Not independently verified at the time of writing:** Gmail sync against a
real mailbox, and hosted vision and transcription. `tests/nebius_live.rs` holds
six live checks for the model path and runs with `--ignored`.

**Not implemented**, and the code says so at run time rather than pretending:
Microsoft Graph, NemoClaw, OpenShell, Hermes Agent.

I would rather be marked down for an honest gap than claim a capability a judge
then finds missing — which is the same standard the assistant itself is held to.

## Feedback on the technologies

*(To complete after the live integration run. Keep it specific and useful —
"Most Valuable Feedback" is a judged category, and vague praise helps nobody.)*

**Nebius Token Factory**
- OpenAI-compatible surface meant the provider client was ~150 lines and the
  agent loop needed no special-casing. Note here: whether tool-call arguments
  arrived as a JSON string or an object, and whether `tool_choice: "auto"`
  behaved as documented.
- Note the actual observed latency and token accounting, and whether
  `/models` was usable as a health check.
- Note any difference between the EU and `us-central1` endpoints.

**NVIDIA Nemotron**
- Note tool-calling reliability over a 75-schema catalogue: did it pick the
  right tool, and did it respect `required` arguments?
- Note whether it honoured the instruction to treat `<untrusted_content>` as
  data when the demo inbox tried to hijack it. This is the single most
  important behaviour for a personal assistant and worth reporting precisely.
- Note how the tiers compared: was the fast model adequate for summarizer and
  researcher subagents?

**Tavily**
- Result quality and `published_date` coverage, which the freshness policy
  depends on.

## Links

- Repository: *(to add)*
- Demo: *(to add)*
- Video: *(to add)*
- License: MIT, visible at the top of the repository

## Pre-existing work

None. FRIDAY was started on 12 October 2026, inside the submission period.
