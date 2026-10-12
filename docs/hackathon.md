# Hackathon requirements — verified

Checked against <https://nebiusglobalaihackathon.devpost.com/> on
**12 October 2026**. Re-verify before submitting; the live page is
authoritative and this is a snapshot.

## Deadline

**Oct 30, 2026 @ 10:30pm IST** as shown on the Devpost page (equivalently
10:00 AM PDT). 18 days from the date of this check.

## Mandatory

> All submissions must run on either Nebius Token Factory or Nebius AI Cloud
> and use at least one NVIDIA open source model. Everything else is up to you.

| Requirement | FRIDAY |
| --- | --- |
| Runs on Nebius Token Factory or AI Cloud | Token Factory, OpenAI-compatible `/chat/completions` with tool calling |
| At least one NVIDIA open source model | `nvidia/nemotron-3-super-120b-a12b` (configurable) |

## Track: Personal AI

The track asks for:

> an always-on, private assistant that works for you while keeping your data
> under your control. Give it persistent memory, reusable skills, access to the
> tools and information you choose, and the ability to carry out tasks across
> your daily workflows.

| Track element | Where it is in FRIDAY |
| --- | --- |
| Always-on | Local service with a scheduler; systemd unit in [deployment.md](deployment.md) |
| Private, data under your control | SQLite on disk, no telemetry, offline mode, `privacy_impact_preview`, local-first OCR and transcription |
| Persistent memory | 5 categories with provenance; survives restart (tested) |
| Reusable skills | `skill.toml` + `SKILL.md` packages, GUI/CLI/file-managed |
| Tools and information you choose | 75 tools behind user-editable permission policy; workspace roots; connector opt-in |
| Tasks across daily workflows | Briefing, scheduling, research, repository inspection, voice and screen |

The track also suggests NVIDIA NemoClaw, OpenShell, Hermes Agent and Nebius
Serverless. None are required. FRIDAY uses none of them today; Serverless Jobs
are the natural fit for guaranteed scheduling and are described as an option in
[deployment.md](deployment.md).

## Submission checklist

| # | Required | State |
| --- | --- | --- |
| 1 | A working project on Token Factory with an NVIDIA model | Implemented; **live model call unverified until the API key is set** |
| 2 | A category | Personal AI |
| 3 | A project description — what, why, how | To write (`docs/submission.md`) |
| 4 | A working demo URL, hosted app, or test build | To do — demo profile ready in [deployment.md](deployment.md) |
| 5 | A public YouTube video, 3 minutes or less, showing it working and how Token Factory + the NVIDIA model are used | To record (`docs/demo-script.md`) |
| 6 | Public repo (GitHub/GitLab/Bitbucket) with an OSS license visible at the top | MIT in [LICENSE](../LICENSE); repo not yet published |
| 7 | README with setup instructions and guidance for running it | [README.md](../README.md) |
| 8 | Highlight NVIDIA/Nemotron usage, where Token Factory helped, and other Nebius tools | README "Hackathon technology" section |
| 9 | Feedback on Token Factory, AI Cloud, and NVIDIA tools used | To write |
| 10 | If the project predates the submission period, explain what changed | Not applicable — started 12 Oct 2026, inside the period |
| 11 | IRL Builders & Brews city, if attended | User to answer |

## Judging criteria, and what each one rewards here

| Criterion | The question asked | FRIDAY's answer |
| --- | --- | --- |
| Technological Implementation | How well is it built, and how effectively does it use Token Factory and Nemotron? | Rust, 140 automated tests, the model drives a bounded agent loop over 75 permission-checked tools and 8 specialist roles |
| Design | A complete, coherent product experience — not a proof of concept | Six-view UI, CLI parity, one configuration model, in-app approvals |
| Potential Impact | A credible, specific case for a real problem and audience | Continuity and verifiability for one developer's daily workflow; evidence instead of claims |
| Quality of the Idea | Creative, non-obvious use, and genuine understanding of the problem space | Proof-Carrying Actions and the online-to-offline knowledge handoff |

## Two findings the project spec did not contain

### 1. "Best Use of Tavily" — $3,000

A separate prize, with three Tavily engineers on the judging panel. FRIDAY
already uses Tavily as its search provider, and the research pipeline is
verified against the live API. Worth making explicit in the submission:
search results are cited per claim, fetch timestamps are stored, a report
cannot be saved with a fabricated or private-address citation, and saved
research stays answerable offline through `prepare_offline_packet`.

### 2. Model tiering is explicitly encouraged

> Reach for Nemotron 3 Ultra when you need serious reasoning, and let Nano or
> Super handle the fast, everyday calls, so your app stays responsive and your
> credits stretch further.

This matters architecturally rather than cosmetically: a parent agent planning
a fan-out does the hard reasoning, while three subagents make many cheap calls.
Tracked as the next implementation step — per-role model selection, so the
planner and the parent can use a larger model while summarizer and researcher
subagents use a faster one.

## Not claimed

- The live Nebius call is **not yet verified**; the suite uses a mock provider
  by design and `tests/nebius_live.rs` is waiting on the key.
- No deployment exists yet, so requirement 4 is open.
- Microsoft Graph, NemoClaw, OpenShell and Hermes Agent are not used.
