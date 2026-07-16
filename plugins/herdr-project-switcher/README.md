# herdr-project-switcher

A portable fuzzy project picker for [Herdr](https://herdr.dev). It changes the
pane you were using to the selected project's directory; it does not create or
switch Herdr workspaces.

The picker, file-tree preview, and README preview are built into one binary.
Herdr is the only runtime requirement. If `zoxide` is available it contributes
frecency ordering. If `git` is available, every project row is colored by its
repository state and shows `+added -removed` line counts plus `?untracked`; the
preview shows Git's native colored status and colors staged, modified, untracked,
and conflicted files in its tree. Both integrations disappear cleanly when
unavailable.

## Install

```bash
herdr plugin install Yassimba/ai-setup/plugins/herdr-project-switcher
```

Managed installs download a checksum-verified binary for macOS, Linux, or
Windows, so users do not need Rust.

For a linked development checkout:

```bash
cargo build --release
mkdir -p bin
cp target/release/herdr-project-switcher bin/
herdr plugin link "$PWD"
```

## Bind a key

macOS and Linux:

```toml
[[keys.command]]
key = "prefix+{"
type = "shell"
command = "herdr plugin pane open --plugin yassin.project-switcher --entrypoint switch"
description = "Project: switch"
```

Windows uses the `switch-win` entrypoint:

```toml
[[keys.command]]
key = "prefix+{"
type = "shell"
command = "herdr plugin pane open --plugin yassin.project-switcher --entrypoint switch-win"
description = "Project: switch"
```

## Project roots

With no configuration, the plugin uses whichever conventional directories
already exist under the current user's home directory: `Projects`, `projects`,
`Developer`, `developer`, `src`, `code`, `dev`, `Documents/projects`, or
`Documents/Projects`. It never recursively scans the entire home directory.

If none contains projects, the picker asks for a root and saves it. Configure
one or more roots explicitly at
`$(herdr plugin config-dir yassin.project-switcher)/config.toml`:

```toml
roots = [
  "~/Projects",
  "~/work/worktrees",
]
```

Each immediate child directory is treated as a project. Missing configured
roots are skipped. On Windows, `~`, `/`, and `\\` are supported in configured
paths.

Windows defaults to PowerShell syntax when sending `cd` to the target pane. If
that pane uses Command Prompt instead, add:

```toml
windows_shell = "cmd"
```

## Keys

| Key | Action |
| --- | --- |
| `enter` | Change the target pane to the selected project |
| `up` / `down` | Move the selection |
| typing | Fuzzy-filter projects |
| `esc` / `ctrl-c` | Close |

The plugin changes only the pane over which it was opened. From a shell it sends
`cd` to that exact pane. From a reviewr sidebar it replaces only that sidebar in
its existing split or tab, rooted at the selected project. Other shells and
reviewr sidebars remain untouched. Unsupported agent/plugin TUIs are refused
without changing anything.
