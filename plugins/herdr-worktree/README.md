# @yassimba/pi-herdr-worktree

Continue the active Pi session in a fresh Herdr-managed git worktree. Ported from the
project-local extension in [herdr](https://github.com/ogulcancelik/herdr) and generalized
to work in any repo Herdr manages.

## Install

```bash
pi install npm:@yassimba/pi-herdr-worktree
```

## Use

Only works inside a Herdr-managed pane (`HERDR_ENV=1`). The extension adds:

- `herdr_start_worktree` — a tool the agent calls to move the current session into a new worktree.
- `/herdr-worktree-start [branch]` — the same flow as a command. Flags: `--branch`, `--base`,
  `--source <checkout>`, `--no-close-pane`.

The flow: create the checkout through [worktrunk](https://worktrunk.dev) (`wt switch`), fork the
live session into the worktree, split the current Herdr pane, start `pi --session` in the new
sibling pane with the worktree as its directory, then shut down the old Pi process and close its
pane. The replacement stays in the same Herdr tab and workspace as the other agents.

## Requirements

Besides a Herdr-managed pane, the `wt` CLI (worktrunk ≥ 0.60) must be on `PATH`.

## Defaults

Worktree creation goes through worktrunk, so its configuration applies:

- **Checkout location** — worktrunk's `worktree-path` template in
  `~/.config/worktrunk/config.toml` decides where checkouts land.
- **Lifecycle hooks** — worktrunk `post-start` hooks run when the worktree is created.
- **Base branch** — the source checkout's current branch (`--base @`) unless you pass `--base`;
  worktrunk shortcuts (`^` for the default branch, `pr:N`, …) work.
- **Existing branches** — a branch that already exists is switched to (`wt switch` without
  `--create`), reusing its worktree if one exists.
- **Source repo** — the extension pins the repo: `wt` runs in pi's working directory (or
  `--source`), so the worktree always branches from the repo the session runs in rather than
  whichever Herdr workspace happens to be focused.
