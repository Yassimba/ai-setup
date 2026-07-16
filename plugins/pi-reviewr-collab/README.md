# pi-reviewr-collab

A trusted [Pi](https://github.com/badlogic/pi-mono) extension that binds a Pi session to the
[reviewr](../herdr-reviewr) pane reviewing the same worktree, over a local, versioned,
target-authenticated protocol.

What it does:

- **Prompt context capture** — when you submit a prompt, the extension snapshots the review
  context from reviewr (review target, current file/line and diff side, the visible patch,
  the selected discussion, and the `C1`/`C2`… context tray) and injects it into the turn.
  The snapshot is taken at submission time, so navigating afterwards never retargets an
  already-asked question.
- **Follow reporting** — read, search, and edit tool locations (plus completed edits with
  their first changed line) stream to reviewr so it can follow the agent's work.
- **Draft staging** — the model gets one extra tool, `stage_review_draft`, which stages
  inline findings and discussion replies as *local* drafts inside reviewr. Nothing on this
  channel can publish, push, approve, resolve, or merge anything; only the reviewer's
  explicit sync does. A draft the reviewer edits becomes theirs — Pi can propose a new one
  but can never overwrite it.

Pi stays fully usable without reviewr: prompts are marked as lacking review context, and
staging fails visibly instead of queueing.

## Rendezvous

Both sides derive the same socket address independently — reviewr from its repo root, the
extension from `git rev-parse --show-toplevel` — hashing `user|worktree` with FNV-1a 64.
A Deep Review workspace pins both ends explicitly instead via the `REVIEWR_COLLAB_SOCKET`
and `REVIEWR_COLLAB_TARGET` pane environment variables. reviewr rejects a hello whose
protocol version or review target does not match; the extension then stays quiet for the
rest of the process.

## Install

```bash
pi install github:Yassimba/ai-setup/plugins/pi-reviewr-collab
```
