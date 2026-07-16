---
name: merge
description: Review-first git merge — preview incoming changes, merge --no-commit --no-ff, resolve conflicts with the user, commit only after review.
disable-model-invocation: true
---

# Merge

Review-first branch integration. The merge is applied but never committed until the user has reviewed it, so every incoming change shows as a colored diff in the editor. Anything that would contaminate that review diff — auto-commits, premature stash pops — waits until the merge commit lands.

**Announce at start:** "I'm using the merge skill to integrate branches."

## Phase 1 — Branch selection

1. If a branch was passed as an argument (`/merge <branch>`), use it and skip to Phase 2.

2. Otherwise detect the current branch and list candidates by recent activity:

   ```bash
   git branch --show-current
   git branch -a --sort=-committerdate \
     --format='%(refname:short) %(committerdate:relative) %(subject)'
   ```

3. Present a dropdown (`AskUserQuestion`) of the top local and remote branches, skipping the current one. Include commit date and last-commit subject so branches are easy to identify.

4. If a remote ref (`origin/foo`) was selected and a local `foo` exists, merge the local branch; otherwise merge the remote ref directly.

5. State the direction: "Merging `<selected>` → `<current>`".

## Phase 2 — Pre-merge safety

6. Working tree check: `git status --short`. If dirty, ask:

   ```yaml
   question: "Working tree dirty. How to handle?"
   header: "Dirty tree"
   options:
     - label: "Stash (Recommended)"
       description: "git stash push -m 'pre-merge: <branch>' — held until the merge commit lands"
     - label: "Commit first"
       description: "Commit the current changes before merging"
     - label: "Abort"
       description: "Stop; deal with the dirty tree manually"
   ```

   The stash stays stashed until the merge is committed — popping earlier would mix the user's changes into the merge review diff.

7. Fetch latest: `git fetch --all --prune`

8. Preview and summarize the incoming changes in 2–3 sentences:

   ```bash
   git log --oneline HEAD..<selected> | head -20
   git diff --stat HEAD...<selected>
   ```

## Phase 3 — Merge

9. ```bash
   git merge --no-commit --no-ff <selected>
   ```

   `--no-commit` leaves the result uncommitted for review; `--no-ff` always creates a merge commit, preserving history. Use `git merge`, not rebase, unless the user explicitly asks for rebase.

10. Clean merge → Phase 4. Conflicts → protocol below. At any point the escape hatch is `git merge --abort` — it restores the pre-merge state losslessly.

## Phase 4 — Review, commit, cleanup

11. Notify: "Merge applied but NOT committed. Changes are in your editor for review. Make edits if needed, then say when to commit."

12. When the user is ready: invoke `/commit` if that skill is installed. Otherwise draft the message (`Merge branch '<selected>' into <current>`, plus a one-line summary of what came in) and confirm via dropdown: Commit / Edit message / Abort merge.

13. After the merge commit lands, pop the stash from step 6 if one was made, resolving any pop conflicts.

14. **Worktree cleanup** — if the merge landed on the repo's default branch (`git symbolic-ref refs/remotes/origin/HEAD`) and `git worktree list` shows a worktree checked out on the merged branch, offer:

    ```yaml
    question: "Merged <branch>. Clean up its worktree?"
    header: "Cleanup"
    options:
      - label: "Yes, clean up (Recommended)"
        description: "git worktree remove <path> && git branch -d <branch>"
      - label: "Keep worktree"
        description: "Clean up manually later"
      - label: "Remove worktree, keep branch"
        description: "Drop the checkout; keep the branch for reference"
    ```

    Run the removal only after explicit approval, and always `git branch -d` (never `-D`) so git refuses if anything is unmerged — that refusal is a signal to keep the branch.

15. Report "merged cleanly" only after a fresh `git status` confirms it.

## Conflict resolution

Classify every conflicted file. `Read` the full file — the conflict markers alone lack context.

- **Trivial** — formatting, import order, whitespace. Resolve automatically with the project's configured formatter.

- **Semantic** — both sides changed logic. First recover intent: read each side's commit messages for the file (`git log --oneline <base>..<side> -- <file>`). Then ask, with real snippets as previews:

  ```yaml
  question: "Semantic conflict in <file>:<line range>"
  header: "Conflict"
  options:
    - label: "Keep ours"
      description: "Current branch version"
      preview: <ours snippet>
    - label: "Keep theirs"
      description: "Incoming branch version"
      preview: <theirs snippet>
    - label: "Combine"
      description: "Merge both intents — proposed combination"
      preview: <proposed combination>
    - label: "Show me both"
      description: "Display full context so I decide manually"
  ```

- **Structural** — file renamed or deleted on one side, modified on the other. Always ask, no recommended default: keep the rename/delete, keep the modification, or show full context — with the consequence of each spelled out.

Every dropdown implicitly has a fifth way out: the user can ask to abort, and `git merge --abort` discards the attempt cleanly.

After resolving, `git add` the resolved paths and show the staged resolution. Conflict resolution is done when `git status` reports zero unmerged paths and every non-trivial resolution was shown to the user.

## Guardrails

- Destructive commands (worktree remove, branch delete, discarding either side of a conflict) run only after explicit approval.
- The merge commit happens only after the user has had the chance to review — that is the point of the skill.
