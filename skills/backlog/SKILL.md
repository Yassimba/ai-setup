---
name: backlog
description: "Backlog management: use when the user mentions a backlog, asks what's next, wants work recorded before implementation, or wants queued ideas or specs prioritized, transitioned, completed, or removed; also use when another skill needs to record lifecycle changes."
---

# Backlog

Use `ai-docs/backlog.md` as the ranked queue of work in flight. Work lands in the Queue before implementation begins. Create the file from the template below on first use.

## Model

Each project appears once in the Queue. Its row points to `spec.md` when that file exists, otherwise `idea.md`, under `ai-docs/plans/<YYYY-MM-DD-project>/`.

- **Type:** `idea` or `spec`.
- **Priority:** P0 is genuinely next; P1 is high value; P2 has no urgency.
- **Status:** `draft → ready → in-progress → review`. Completing a reviewed project moves it out of the Queue and into Completed.
- **Rank:** sort by priority and preserve the user's order within each tier.

```markdown
# Backlog

## Queue

| #   | Type | Priority | Doc | Status | Summary |
| --- | ---- | -------- | --- | ------ | ------- |

## Completed

| Doc | Completed | Branch |
| --- | --------- | ------ |
```

## Invariants

- Keep at most one P0.
- Keep one row per project and promote its pointer from `idea.md` to `spec.md` when the spec appears.
- Keep ranks contiguous.
- Keep completed work in Completed rather than the Queue.
- Preserve every design file when changing or removing its backlog row.

After every mutation, reread `ai-docs/backlog.md` and verify every invariant before reporting completion.

## Branches

A branch is complete when its criterion holds and, after a mutation, the invariant check has passed.

| Invocation                      | Action                                                                                                                                                                                                                                 | Complete when                                                                    |
| ------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------- |
| `/backlog`                      | Show the Queue and five most recent Completed rows. Check the top three Queue rows against disk for missing docs, stale type or pointer, and duplicate projects.                                                                       | The requested rows are shown and every detected drift is named.                  |
| `/backlog add <path>`           | Accept an existing `idea.md` or `spec.md`; read its opening section for the summary; ask for P0, P1, or P2; then insert it at the end of that tier or update the project's existing row.                                               | The row is persisted in the chosen tier with the doc's summary.                  |
| `/backlog next`                 | Return the first `ready` row in Queue order. When none is ready, summarize projects by status and name the nearest useful transition; for drafts, suggest `/grill-me`, then `/backlog status <name> ready` after shared understanding. | One ready project is selected or every reason nothing is selectable is reported. |
| `/backlog reprioritize`         | Show the complete current ordering, ask for all desired changes in one prompt, and apply them.                                                                                                                                         | The persisted tiers match the changes the user confirmed.                        |
| `/backlog status <name> <new>`  | Resolve the project unambiguously and set its status to `<new>` when the lifecycle allows the transition; otherwise report why not. A reviewed project may return to `in-progress` when changes are requested.                         | The persisted status is `<new>`, or the disallowed transition is reported.       |
| `/backlog done <name> <branch>` | Resolve a project in `review`, remove it from the Queue, and append it to Completed with the session date and merge branch.                                                                                                            | The Completed row records both the date and the branch.                          |
| `/backlog remove <name>`        | Resolve the project and remove its Queue row.                                                                                                                                                                                          | The project is absent from the Queue.                                            |
