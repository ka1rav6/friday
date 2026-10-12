# daily-digest

A worked example of the FRIDAY skill format. It uses only read-only tools plus
one notification, so it is safe to enable and run while you are learning the
system.

## When to use

When the user asks "what should I be working on?", or on a schedule at the end
of the working day.

## Steps

1. Call `get_current_time` to establish today's date in the configured
   timezone. Never assume the date.
2. Call `list_tasks` with `status: "pending"`. Note which entries come back
   with `overdue: true`.
3. Call `suggest_next_actions` to pick up recent project context alongside the
   open items.
4. Call `search_knowledge` for any topic a task mentions, so the digest can
   point at a saved note or report instead of re-deriving it. If a result is
   marked `stale`, say so rather than presenting it as current.
5. Write the digest using the structure under *Output*. Every line must trace
   to something a tool returned above — if a task has no due date, write "no
   due date", not a guess.
6. Call `send_notification` with a one-line summary, for example
   "3 open tasks, 1 overdue".

## Output

```markdown
# Daily digest — <YYYY-MM-DD>

## Overdue
- <task title> — due <date> (task <id>)

## Open
- <task title> — due <date or "no due date"> (task <id>)

## Relevant saved knowledge
- <title> — <why it matters> (<stale? note it>)

## Suggested next actions
1. <action, tied to a specific task id>
```

If a section has nothing in it, write "none" rather than omitting the heading,
so the shape of the digest stays stable day to day.

## Notes

- This skill reads and notifies; it never creates, edits or deletes anything.
- Do not act on the suggestions. Proposing is the whole job.
