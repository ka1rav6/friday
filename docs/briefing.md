# Morning briefing and scheduling

## What the briefing is

A dated, prioritized report built from new mail, today's calendar, open tasks
and extracted deadlines — with a source link on every item.

The split matters: **collection and extraction are deterministic Rust;
synthesis is the model's job.** `get_morning_briefing` gathers the inputs and
returns them with instructions; the agent writes the prose. Dates are never
invented because they are never generated.

## Connectors

| Provider | State |
| --- | --- |
| `synthetic` | Implemented. A fixed, clearly-labelled demo dataset. |
| `gmail` | **Not implemented.** Selecting it returns an error. |
| `microsoft` | **Not implemented.** Selecting it returns an error. |

Setting `briefing.email_provider = "gmail"` fails with a message saying the
connector is not implemented and that FRIDAY will not present synthetic data as
real email. That refusal is deliberate and tested.

Every synthetic row carries `is_synthetic: true` and a
`friday://synthetic/email/<id>` link, and the briefing is instructed to say so.

### Adding a real connector

Implement OAuth in `briefing::sync` for your provider and keep the existing
guarantees: read-only scopes, least privilege, refresh tokens in the OS
keyring (never in the config file), incremental sync, deduplication on
`(provider, external_id)`, a link back to the original message, and a clear
error when access is revoked. Bodies must continue to pass through
`util::untrusted_block`.

## Deadline extraction

`briefing::extract_deadlines` reports candidates with the evidence they came
from and an explicit certainty:

| Certainty | Meaning |
| --- | --- |
| `explicit` | An ISO date was present in the text. Reliable. |
| `relative_needs_confirmation` | A weekday reference like "this Friday". **Not resolved.** |
| `explicit_time_only` | A time with no date. |

Text with no dates yields no candidates. A relative reference is never turned
into a date silently — `cross_source_deadline_verifier` compares mail,
calendar and tasks and reports agreements, items in mail that nothing tracks,
and relative references still needing confirmation.

## Prompt injection

Message bodies reach the model wrapped in `<untrusted_content>`. In addition,
`gather` scans for known override phrasings and returns `injection_warnings`,
and the instruction block tells the agent to report them and not act on them.
The synthetic inbox contains one such message on purpose, and a test asserts it
is surfaced.

## Scheduling

```bash
friday brief                       # run now
friday brief --force               # run again today
curl -X POST localhost:8787/api/jobs/morning-briefing/run
```

The scheduler ticks once a minute and fires any job whose `next_run_at` has
passed.

- **Timezone-aware.** `next_run` resolves `HH:MM` in the job's IANA timezone
  and steps forward across a DST gap rather than producing a nonexistent
  instant.
- **Idempotent per local date.** Each run takes an idempotency key of
  `<job_id>:<local date>`. A duplicate run returns `skipped: true` with the
  reason; a previously *failed* run is allowed to retry. So one briefing per
  day, no duplicates.
- **Only registered, enabled skills run.** `schedule_job` refuses anything
  else, so a schedule cannot execute an arbitrary command.
- **Recovery.** `scheduled_workflow_recovery` lists overdue jobs and runs that
  have been `running` for over an hour, with a safety verdict for rerunning and
  the likely cause.

### The honest limitation

This is a **local** scheduler. It only fires while the FRIDAY process is
running. It cannot deliver a 07:30 briefing if the machine was asleep — and it
says so, in the `schedule_job` result, in the recovery report and here.
Guaranteed delivery needs a cloud scheduler (for example a Nebius Serverless
Job calling `POST /api/jobs/morning-briefing/run` against a hosted instance).
That is a deployment choice, not something the local scheduler will pretend to
do.

## Built-in skill

`morning-briefing` ships enabled, in the `builtin` scope, and appears in the
same library as your own skills. Its instructions are ordinary Markdown: read
them with `friday skills list`, and write your own variant by copying the
pattern.
