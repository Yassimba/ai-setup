# herdr-reviewr

A code-review sidebar for [herdr](https://herdr.dev). Your agent writes the code. You read the
diff in a pane next to the chat, comment on the lines, and send the notes back. You never leave
the terminal.

It reviews local work (uncommitted, branch, and last-turn diffs) and remote work (GitHub pull
requests and GitLab merge requests, including self-hosted and Enterprise, detected from
`origin`). It runs on macOS, Linux, and Windows, and installs prebuilt — no Rust toolchain
needed:

```bash
herdr plugin install Yassimba/ai-setup/plugins/herdr-reviewr --yes
```

What you get in one pane pointed at a git worktree:

- **Diff review** — the agent's changed files, syntax-highlighted, scoped to *uncommitted*,
  *branch*, or *last turn*.
- **Line comments** — select lines and write a note. It stays visible as a card under the code.
- **Send** — one key drops every comment into the agent's input as `path:start-end — comment`.
- **File viewer** — browse the whole worktree, not just the diff.
- **PR/MR review** — read checks and full comment threads, reply locally, and post everything
  as one review when you say so.
- **Deep Review** — one key opens a workspace with a [Pi](https://github.com/badlogic/pi-mono)
  agent under the review. You pick which comments it sees, watch it work, and edit the reply
  drafts it writes. Nothing posts until you sync.
- **Project switcher** — `Ctrl+P` points the sidebar at another project. Built in.
- **Themes** — 18 palettes in dark and light: Catppuccin, Dracula, Nord, Gruvbox, Tokyo Night,
  Rosé Pine, Solarized, and more.

reviewr never edits your worktree and never sends anything on its own. Its only write to git is
a private baseline ref under `refs/reviewr/` (Deep Review adds a private branch, in its own
worktree). Drafts reach the forge only when you sync.

## Requirements

- **herdr ≥ 0.7.0** (the plugin system).
- **git** on `PATH`.
- A **truecolor (24-bit)** terminal that can draw Unicode box characters. Pick a theme that
  matches its light or dark background (see [Theme](#theme)).
- **macOS, Linux, or Windows** (Windows needs herdr's Windows beta).
- **`gh`** (GitHub CLI) and/or **`glab`** (GitLab CLI), logged in. Optional — only the **PR**
  tab uses them. Everything else works without them.
- **`pi`** with a companion extension. Optional — only **Deep Review** uses it (see
  [Deep Review](#deep-review-an-agent-inside-the-review)).

## Install

From the herdr marketplace. You get a prebuilt binary:

```bash
herdr plugin install Yassimba/ai-setup/plugins/herdr-reviewr
```

The sidebar **opens by itself for every new worktree**, so installing is enough. Set
`auto_open = false` to keep it hidden until you ask (see [Configuration](#configuration)). To
open it on demand, bind a key to the **reviewr: toggle sidebar** action in your herdr config.
Keybindings live in your own config, not in the plugin:

```toml
[[keys.command]]
key = "cmd+r"
type = "plugin_action"
command = "yassimba.reviewr.toggle"   # <plugin_id>.<action_id> — the id, not the name
```

`cmd+…` chords reach herdr. macOS swallows `alt+…`. On Windows the action ids end in `-win`:
bind `yassimba.reviewr.toggle-win`. With no key bound, run the action directly:
`herdr plugin action invoke toggle --plugin yassimba.reviewr`.

There are two more actions, meant for scripts and layout plugins. `open` opens the sidebar and
does nothing if one is already open. `close` closes it and does nothing if none is. Bind or
invoke them the same way, as `yassimba.reviewr.open` and `yassimba.reviewr.close`. See
[Auto-open and layout plugins](#auto-open-and-layout-plugins) for the layout recipe.

## Your first review

The core loop takes five keys. Open the sidebar next to your agent and:

1. **Pick a file.** The agent's changed files are in the right pane. `j` / `k` moves the cursor.
   The diff opens on the left as you go.
2. **Focus the diff.** Press `Tab`.
3. **Select the lines.** Press `v`, then `j` / `k` to extend (or click-drag).
4. **Comment.** Press `c`, type your note, `Enter` to save. It stays on screen as a card under
   the line.
5. **Send.** Press `s`. Every comment lands in the agent's input as `path:start-end — comment`.
   You add context and hit enter.

The footer always shows the keys that work right now, so you learn it by using it. The mouse
works too: click a file, drag to select lines, click a tab or the `Send` button, scroll. The
sections below walk through each workflow; [Controls](#controls) has every key.

## Reviewing local changes

The **Changes** tab (key `1`) lists the changed files for the active scope, with `+/-` stats.
Pick a file, read its diff, select lines, comment. Three scopes decide what counts as changed:

- **uncommitted** (`u`) — the working tree vs `HEAD`: staged, unstaged, and untracked.
- **branch** (`b`) — the working tree vs where your branch left the base branch. The default
  base is `origin/main`, then `origin/master`, `main`, `master`; change it with `base_branches`
  or `--base` (see [Base branch](#base-branch)). This scope is **uncommitted** plus the
  branch's commits.
- **last turn** (`t`) — only what the agent changed since its latest turn started (see
  [Limitations](#limitations)).

Every scope respects `.gitignore`, so build output never clutters **Changes**. To review a
file, track it in git. A file you ignore on purpose but still want reviewed (a plan, a sample
env) belongs in the repo: it shows as a change until committed, then drops out on its own.

When you're done, `s` sends everything to the agent, or `y` copies it all to the clipboard.

## Browsing the whole worktree

The **All files** tab (key `2`) shows the whole tree, not only what changed. Any file's current
content renders in the diff pane, with small gutter marks for lines the active scope touched.
Ignored paths show too, dimmed. A directory that is ignored as a whole (`target/`,
`node_modules/`) is one collapsed row that only loads when you expand it. You can comment here
as well.

## Reviewing a PR or MR

The **PR** tab (key `3`) shows the branch's open pull request (GitHub, via `gh`) or merge
request (GitLab, via `glab`). reviewr picks the provider by looking at `origin`. You get:

- **The state** — draft, open, merged, or closed, whether it can merge, and whether you have
  unpushed commits.
- **The checks** — CI runs on GitHub, pipeline jobs on GitLab, with a pass/fail rollup.
- **The comments** — reviews, inline findings, and plain comments, newest first, with
  `resolved` and `outdated` markers. Threads load in full, not just the first page. If one
  can't finish loading, it says so and offers a reload — you never get a silently shortened
  thread.

The selected thread shows the code around it and its replies. `c` drafts a reply. `s` posts all
pending drafts as one review. `o` opens the PR in your browser.

By default the tab shows the PR/MR of the branch you're on. Press `p` to look at any other: the
picker lists open ones first, then the newest merged and closed with their fate
(`merged · ✓ passed`, `merged · ✗ failed`, `closed`) and comment counts. Pick one and the
**Changes** tab shows that review's files and diff — without checking out its branch. Its
threads sit under their lines; select lines and press `c` to draft a comment, `s` to post the
group. `esc` goes back to your own branch.

reviewr never checks out the branch, fetches into your repo, resolves threads, re-runs checks,
or merges.

## Deep Review: an agent inside the review

Reading a review is one thing. Often you want an agent *in* it: answering "does this comment
still apply?", making the requested change, drafting the replies. Deep Review opens a workspace
for exactly that — reviewr on top, a fresh [Pi](https://github.com/badlogic/pi-mono) agent
below, both on the review's exact code.

### One-time setup

Deep Review talks to Pi through a companion extension. Install it once:

```bash
pi install github:Yassimba/ai-setup/plugins/pi-reviewr-collab
```

To pick the model that Pi runs (otherwise it uses Pi's own default):

```toml
# ~/.config/herdr/plugins/config/yassimba.reviewr/config.toml
deep_pi_model = "codex/gpt-5.6-sol"
```

### Start a session

Press **`Shift+D`**. From a local view — the Changes or All files tab — the session targets
this worktree's checked-out branch and its local changes, even when the branch has an open PR. Only a review you
are actually looking at becomes the target: the PR tab, a pinned pick, or a highlighted row
in the `p` picker. reviewr then:

1. **Checks out the review's code.** For a remote PR/MR it fetches the exact head and checks it
   out on a private branch (`reviewr/pr-N` or `reviewr/mr-N`) in a separate worktree under
   reviewr's own state directory. Your checkout stays untouched. A local review uses the
   current repo as is.
2. **Builds the workspace.** One tab: reviewr on top, an interactive Pi below, both in that
   worktree. The Pi pane gets focus so you can type the first question. `Shift+D` on the same
   review later brings you back to the existing workspace instead of making a second one.

### Work with the agent

**Pick what Pi sees.** Put the cursor on a comment and press `a` ("ask pi"): that comment
becomes Pi's context and focus jumps to the Pi pane. `Shift+A` adds or removes a comment
without leaving reviewr. Picked comments show as chips in the footer, like `C1 C2 ✦ pi`.
reviewr copies the whole thread the moment you pick it, so `C1` keeps meaning the same thing
even after the review moves on. Every prompt you send Pi carries the review, your current file
and line, the visible patch, and those comments.

**Watch it work.** As Pi reads and edits, the diff view follows it. Navigate yourself and it
backs off. `f` turns following off; the footer then still shows where Pi is
(`✦ pi ⊘follow @ src/a.rs:12`) without moving your view. Every line Pi has changed since the
session started carries a `✦` badge in the gutter, so its edits stand apart from the review's
own changes — your pre-session local edits are never marked, and a `U` update re-anchors the
badges so the forge's fresh commits aren't pinned on Pi.

**Step through its edits.** `⌘←` / `⌘→` walk back and forward through Pi's past edits, with
`Alt+←` / `Alt+→` and `Ctrl+O` / `Ctrl+I` as equivalents for terminals whose encoding can't
carry those chords. The footer always shows where you are: `⟲ 14/14` while live at the newest
edit, `⟲ 3/14` while browsing. Stepping into history turns following off, so Pi can't yank the
view away while you look around; stepping forward past the newest edit turns it back on — going
back and coming forward returns you exactly to following live. The history is one linear log
for the whole session — an edit landing while you browse appends (`⟲ 14/15`, `14/16`…) without
moving your place, and nothing is ever truncated. Press `f` any time to re-follow and jump
straight back to live.

**Edit its drafts.** Pi can write inline findings and thread replies — as local drafts only,
shown as cards headed `pi draft` so you always know whose words you're reading. The moment you
edit one (`e`), it's yours: Pi can propose a new draft but can never change what you touched.
Delete (`d`), send (`s`), and sync work exactly like your own comments. Nothing posts until you
sync.

**Keep up with new pushes.** If the PR gets new commits on the forge, reviewr offers `U`: a
fast-forward when that's clean, otherwise a rebase that stops and rolls back on conflict. It
never resolves anything for you.

### Pause, resume, end

reviewr saves the session to disk, per review: drafts, picked comments, follow state, edit
history. Close the workspace and nothing is lost — `Shift+D` on the same review resumes it.
(The Pi *conversation* may be gone; the review context comes back.) "Same review" means the
same PR/MR, or locally the same branch: `Shift+D` after switching branches starts a fresh
session, and the old branch's session sits parked until you check it out again. A per-review
lock keeps two reviewr processes from posting the same feedback twice.

**`Shift+X`** ends the session. The first press tells you exactly what you'd lose — unsynced
drafts, uncommitted edits, local commits. The second press deletes the saved session, removes
the worktree and branch reviewr created, and closes the workspace.

Deep Review needs to run inside a herdr session. If another reviewr already serves the same
worktree, the agent link stays off and plain reviewing continues.

## Switching projects

`Ctrl+P` opens a project picker inside the sidebar. Type to filter, `↑` `↓` (or
`Ctrl+N` / `Ctrl+P`) to move, `enter` to switch, `esc` to close. Picking a project points the
running sidebar at that repo — a fresh review session, no close-and-reopen — and touches
nothing else: no other pane is focused, typed into, or closed. If you still have unsent
comments, the switch asks for a second `enter` before dropping them.

By default the picker lists the current repo's siblings — for a repo at `~/code/myapp`,
everything in `~/code/`. Point it somewhere else with `switcher_roots`:

```toml
# ~/.config/herdr/plugins/config/yassimba.reviewr/config.toml
switcher_roots = ["~/Documents/projects", "~/work"]   # default: the repo's parent
```

With [zoxide](https://github.com/ajeetdsouza/zoxide) on your `PATH`, the list is ordered by the
folders you actually visit most; without it, alphabetically. Hidden directories are skipped.

## Controls

**Getting around**

| Key | Action |
| --- | --- |
| `1` `2` `3` | Switch tab — Changes / All files / PR |
| `u` `b` `t` | Switch scope — uncommitted / branch / last turn |
| `j` `k` · `↑` `↓` | Move the cursor in the focused pane |
| `PageUp` `PageDown` | Move a page |
| `Ctrl+U` `Ctrl+D` | Move a half-page |
| `Tab` | Switch focus between the file list and the diff |
| `→` `←` | Expand or collapse a directory or fold, or scroll the diff sideways |
| `w` | Toggle line wrap |
| `]` `[` | Widen / narrow the file list |
| `Ctrl+P` | Switch project |
| `r` | Refresh now |
| `q` | Quit |

**Reviewing** (in the diff)

| Key | Action |
| --- | --- |
| `v` | Start a line selection, then `j` / `k` to extend (or click-drag) |
| `c` | Comment on the selection — or on the current line |
| `e` `d` | Edit / delete the comment under the cursor |
| `n` `N` | Jump to the next / previous comment |
| `l` | List every comment |
| `s` | Send all comments to the agent |
| `y` | Copy all comments to the clipboard |
| `esc` | Clear the selection |

**In the comment box**

| Key | Action |
| --- | --- |
| `Enter` | Save the comment |
| `Esc` | Cancel |
| `Shift+Enter` · `Alt+Enter` · `Ctrl+J` | Insert a newline |

Plus the usual caret moves: arrows, `Home` / `End`, `Ctrl+A` / `Ctrl+E`, word-jump with
`Alt+b` / `Alt+f`, and `Ctrl+W` / `Ctrl+U` / `Ctrl+K` to delete by word or to the line edge.

**PR/MR tab**

| Key | Action |
| --- | --- |
| `j` `k` | Move through checks and comments |
| `PageUp` `PageDown` | Scroll the selected comment |
| `o` | Open the PR in your browser |
| `p` | Open the PR/MR picker; type a number, title, branch, author, or state, then `enter` to pin |
| `c` | Draft a reply to the selected comment |
| `s` | Post all pending drafts for this PR/MR as one review |
| `esc` | Unpin — back to your own branch's PR/MR |
| `r` | Refresh |

**Deep Review**

| Key | Action |
| --- | --- |
| `Shift+D` | Start or resume Deep Review for the active target or the highlighted picker row |
| `a` | Ask Pi — make the comment under the cursor Pi's context and focus the Pi pane |
| `Shift+A` | Add or remove the comment under the cursor from Pi's context |
| `f` | Toggle follow mode |
| `⌘←` `⌘→` | Step back / forward through Pi's edits (also `Alt+←`/`Alt+→` and `Ctrl+O`/`Ctrl+I`; forward past the newest edit resumes following) |
| `U` | Pull in new commits pushed to the review — fast-forward or rebase |
| `Shift+X` | End Deep Review (press twice; the first press lists what you'd lose) |

## Configuration

CLI flags on the pane command:

| Flag | Default | Meaning |
| --- | --- | --- |
| `--poll <ms>` | `2000` | worktree poll interval (min `200`) |
| `--base <ref>` | auto | base branch for `branch` scope, overrides `base_branches` |
| `--theme <name>` | `catppuccin` | UI + syntax theme (see below) |
| `--wrap <on\|off>` | `on` | soft-wrap long diff lines (`w` toggles at runtime) |

Everything else lives in reviewr's own config file:

```text
~/.config/herdr/plugins/config/yassimba.reviewr/config.toml
```

Create the file if it doesn't exist yet. Note that this is reviewr's file, not herdr's:
settings added to herdr's own `~/.config/herdr/config.toml` never reach reviewr.

The file takes these nine keys:

```toml
theme = "tokyo-night"
base_branches = ["origin/develop", "origin/main", "main", "master"]
toggle_placement = "overlay"
toggle_direction = "down"
auto_open = false
github_host = "github.example.com"
gitlab_host = "gitlab.example.com"
switcher_roots = ["~/Documents/projects"]
deep_pi_model = "codex/gpt-5.6-sol"
```

The host keys are for edge cases only. The PR tab detects the provider from `origin`:
`github.com` and `gitlab.com` just work, and any other host works once the matching CLI is
logged in to it (`gh auth login --hostname …` / `glab auth login --hostname …`) — reviewr reads
the hosts the CLIs already know. Set `github_host` / `gitlab_host` only when neither CLI knows
the host yet, or when both claim it. GitLab origins support nested subgroups.

A missing file or omitted key uses its default. One bad key makes the whole file invalid:
reviewr applies none of it, the sidebar shows the error, and the plugin's actions fail rather
than guess. Fix the file and the running sidebar recovers on its next refresh. If your editor
or config manager might save the file in two steps, have it write a temp file and rename it
into place, so reviewr never reads a half-written config.

### Theme

One theme colors everything, chrome and syntax together. Set it in the config file. reviewr
re-reads the file on refresh, so you can change themes without a relaunch:

```toml
# ~/.config/herdr/plugins/config/yassimba.reviewr/config.toml
theme = "tokyo-night"
```

`--theme` overrides the config file (handy for a dev run). Pick a name that matches your
terminal's background: the pane keeps that background, so a light theme on a dark terminal
reads poorly, and the reverse too. Available:

- **Dark:** `catppuccin`, `catppuccin-frappe`, `catppuccin-macchiato`, `dracula`, `nord`,
  `gruvbox`, `one-dark`, `solarized`, `monokai`, `tokyo-night`, `rose-pine`.
- **Light:** `catppuccin-latte`, `gruvbox-light`, `one-light`, `solarized-light`, `github-light`,
  `tokyo-night-day`, `rose-pine-dawn`.

Names match herdr's where both ship a palette. An unknown name in the config is an error. The
standalone `--theme` dev flag keeps its old fallback to `catppuccin`.

### Base branch

The **branch** scope diffs against a base branch. reviewr tries an ordered list of candidates
and uses the first one that exists in your repo, so one setting works across repos with
different trunks. The default list is `origin/main`, `origin/master`, `main`, `master`.

To review against something else — a `develop` trunk, say — set `base_branches`:

```toml
# ~/.config/herdr/plugins/config/yassimba.reviewr/config.toml
base_branches = ["origin/develop", "origin/main", "main", "master"]
```

reviewr re-reads the file on refresh, so edit it and press `r`; no relaunch. A `--base <ref>`
flag still wins when it names a ref that exists. A bad value blocks the plugin like any other
invalid config.

### GitHub hosts

GitHub.com works without configuration. To read pull requests from a GitHub Enterprise host,
set its bare hostname:

```toml
github_host = "github.example.com"
```

reviewr matches that exact origin host, or an SSH alias that starts with
`github.example.com-`, such as `git@github.example.com-work:owner/repo.git`. The alias rule
applies only to SSH-style origins; HTTPS hosts must match exactly. GitHub.com keeps working
when Enterprise is configured.

The host comes from origin's fetch URL, after Git applies any `url.*.insteadOf` rewrite. A
different push URL changes nothing, and every API call names the host explicitly, so `GH_HOST`
can't redirect one. Log the CLI in to the Enterprise host with
`gh auth login --hostname github.example.com`.

### Sidebar placement

By default the toggle opens reviewr as a split to the right of your agent. Change that with
`toggle_placement`. reviewr re-reads the file on every toggle, so a change applies the next
time you press the key.

```toml
# ~/.config/herdr/plugins/config/yassimba.reviewr/config.toml
toggle_placement = "overlay"   # split | overlay | zoomed | tab   (default: split)
toggle_direction = "down"      # right | down — split only        (default: right)
```

- **`split`** sits next to your agent and leaves the keyboard with it. `toggle_direction` puts
  reviewr on the right (default) or below.
- **`overlay`** covers the whole tab with reviewr and gives it the keyboard. Toggle again to
  drop back to your agent.
- **`zoomed`** fills the tab like overlay and gives reviewr the keyboard.
- **`tab`** opens reviewr in its own tab and gives it the keyboard.

On a new worktree, reviewr auto-opens only for `split` and `tab`. With `overlay` or `zoomed`
it stays out of the way until you press the toggle. An unrecognized value makes the config
invalid. You can also turn auto-open off entirely — next section.

### Auto-open and layout plugins

reviewr opens itself for every new worktree by default. To make it wait for your toggle key,
set:

```toml
# ~/.config/herdr/plugins/config/yassimba.reviewr/config.toml
auto_open = false   # default: true
```

Do this when another plugin arranges your new worktrees, for example
[herdr-plus](https://github.com/cloudmanic/herdr-plus) worktree layouts. Both plugins react to
the same worktree event and race each other. The race can skip the layout entirely, or drop
reviewr as a split in the middle of it. With `auto_open = false`, reviewr leaves fresh
workspaces alone: the layout builds undisturbed, and your toggle key opens reviewr on top of
it, in whatever placement you configured.

A layout can also open reviewr itself, once its panes are in place:

```
herdr plugin action invoke open --plugin yassimba.reviewr
```

`open` ignores `auto_open`, because an explicit call is you asking. It uses your configured
placement and does nothing when a sidebar is already open, so a layout can run it on every
pass. Two things to know. The action opens reviewr in the **focused** workspace, so invoke it
while the new workspace has focus. And it opens reviewr as its **own new pane** — a layout pane
whose command *is* the invoke will exit as soon as the invoke returns. Run it as a one-shot
command from your layout hook, not as a pane that should stay.

## Limitations

This is a focused, young tool. The known constraints:

**Terminal & theme**
- **Truecolor required** — colors are 24-bit RGB with no 256/8-color fallback. Basic terminals
  render wrong colors.
- **The theme must match the terminal** — the pane keeps the terminal's background, so a light
  theme on a dark terminal reads poorly, and the reverse too. There is no automatic light/dark
  detection yet; you set it by hand.
- **Add / remove are red / green** — no second cue for colorblind users yet.
- **Unicode box characters required** — no Nerd Font needed, though.

**Platform**
- **macOS, Linux, and Windows.** Windows needs herdr's Windows beta, and the action ids end in
  `-win` there (see [Install](#install)).
- **Clipboard copy** uses `pbcopy` on macOS, or `wl-copy` / `xclip` / `xsel` on Linux. With
  none installed (including on Windows, for now) it says so, and **Send** still works. OSC 52
  and Windows clipboard are on the roadmap.

**herdr coupling**
- **Send needs a findable agent pane** — the agent in your tab, or the only agent in the
  workspace. Otherwise Send does nothing and keeps your comments. Browsing and diffing work
  without herdr.
- **last turn works by polling** (every 2 s by default) — a turn that starts and finishes
  between two polls never gets its own snapshot. The scope then shows everything since the last
  turn start reviewr *saw*. Never lines the agent didn't write, but possibly more than one
  turn. In a Deep Review workspace, Pi reports its turns directly, so the scope is exact there.

**PR/MR review**
- Needs a logged-in provider CLI (`gh` or `glab`) matching `origin`. Without one, the tab
  explains what to fix; local review keeps working.
- Threads load fully, up to a fixed number of pages per surface. A thread that hits that limit,
  or loses a page to a network error, is marked partial with the reason and a reload marker —
  never silently cut. Remote file lists stop at 100 files, marked when cut off.
- GitHub takes new inline comments as one review, but thread replies go through a different
  API, one by one. On GitLab, reviewr stages its own draft notes and publishes them in one go —
  publish or delete draft notes of your own first, so reviewr can't sweep them up.

**Review model**
- **In a plain sidebar, comments and drafts live in memory only** — closing the pane loses
  whatever you haven't sent or synced. **Deep Review is the exception:** its sessions are saved
  to disk and `Shift+D` resumes them (the Pi conversation itself may not survive).
- **Local Send is all-or-nothing.** Remote sync reports per-item results where GitHub forces
  separate calls; posted drafts leave the queue, definite failures stay retryable. If a post
  *may* have reached the forge but the response got lost, reviewr marks it unknown and won't
  blindly retry — check the forge first.
- **No line-number tracking across rebases** — a comment is found again by its diff snippet,
  not its line number. A comment that no longer matches is flagged stale, not dropped.
- **One sidebar per worktree** — two on the same worktree fight over the baseline ref, and the
  last writer wins. Same for the agent link: a second reviewr on the worktree runs with
  collaboration off.

**Deep Review**
- Needs a herdr session, the `pi` CLI, and the `pi-reviewr-collab` extension. Without them,
  `Shift+D` explains what's missing; everything else works.
- reviewr and the extension must speak the same protocol version — a mismatched extension goes
  quiet instead of half-working. Keep both sides current.

**Budgets**
- Files over **2 MB** or **50,000 lines** show a "too large" notice. **Binary** files get no
  diff.

## Building from source

For contributors. `herdr plugin link` skips the download step, so put a locally built binary
where the pane looks for it, at `$HERDR_PLUGIN_ROOT/bin/herdr-reviewr`:

```bash
git clone https://github.com/Yassimba/ai-setup
cd ai-setup/plugins/herdr-reviewr
just install   # build release → bin/herdr-reviewr, ad-hoc re-signed on macOS
herdr plugin link .
```

`just install` replaces the binary with a fresh file and re-signs it. On Apple Silicon that
matters: overwriting a signed binary in place breaks its signature, and macOS then kills it at
launch. A plain `cp target/release/herdr-reviewr bin/` makes the pane open and instantly close.

**The dev loop** after the first link:

1. Edit the code.
2. `just install` to rebuild and re-sign the binary under `bin/`.
3. Toggle the sidebar off and back on. The open pane keeps running the *old* process until
   then, so a rebuild alone changes nothing on screen.

This loop only works while the plugin is **linked**, not installed from the marketplace. Check
with `herdr plugin list`. A `github:…` source means the pane runs a *downloaded* binary under
`~/.config/herdr/plugins/github/`, where local rebuilds never land. Switch a GitHub install to
a dev link:

```bash
herdr plugin uninstall yassimba.reviewr   # config is keyed by id and survives
herdr plugin link .
```

## Roadmap

Customizable keybindings, structured (JSON) export, in-diff search, a side-by-side split view,
mark-file-reviewed, automatic light/dark theme detection, more themes (`kanagawa`, `vesper`,
`everforest`, `ayu`, a dark `github`), a `terminal`-following palette, and OSC 52 clipboard.

## License

[MIT](LICENSE). reviewr started from
[persiyanov/herdr-reviewr](https://github.com/persiyanov/herdr-reviewr) (vendored at its
v0.11.0, MIT) and has been developed independently since — see the
[changelog](CHANGELOG.md) for where the lines diverge.

Syntax highlighting comes from [syntect](https://github.com/trishume/syntect) and
[two-face](https://github.com/CosmicHorrorDev/two-face). Most themes' syntax colors come from
two-face's bundled set.

Bundled `.tmTheme` syntax files in `assets/`, each under its own license:

- [Catppuccin Mocha](https://github.com/catppuccin/bat) — MIT.
- [Tokyo Night](https://github.com/folke/tokyonight.nvim) (`tokyo-night`, `tokyo-night-day`) — Apache-2.0.
- [Rosé Pine](https://github.com/rose-pine/tm-theme) (`rose-pine`, `rose-pine-dawn`) — MIT.
