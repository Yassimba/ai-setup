# @yassimba/pi-ask-user

A reviewed distribution of [pi-ask-user](https://github.com/edlsh/pi-ask-user)
for the Yassimba setup catalog. This package contains the exact upstream npm
release recorded in [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md). It adds no
runtime code of its own.

## Install

Select **Ask User** with `ai-setup add`, or install it directly:

```sh
pi install npm:@yassimba/pi-ask-user
```

Restart Pi or run `/reload`. The package provides one tool:

- `ask_user` presents structured questions in a searchable split-pane TUI with
  single- or multi-select options (`allowMultiple: true`), freeform input, and
  a details preview pane.

The package also includes upstream's `ask-user` skill, which teaches the agent
when to gate decisions on a question. Display behavior can be tuned with the
`PI_ASK_USER_DISPLAY_MODE`, `PI_ASK_USER_OVERLAY_TOGGLE_KEY`, and
`PI_ASK_USER_COMMENT_TOGGLE_KEY` environment variables as described in the
[upstream README](https://github.com/edlsh/pi-ask-user#readme).

Pi extensions run with your user account's permissions. Review the upstream
source before enabling it.

## Updating the bundled release

1. Inspect the new upstream npm tarball, source commit, dependencies, and licenses.
2. Set an exact `pi-ask-user` version in `package.json`.
3. From `plugins/ask-user`, run
   `npm install --package-lock-only --ignore-scripts --omit=peer --install-strategy=nested --workspaces=false`
   to update `npm-shrinkwrap.json`.
4. Update `THIRD_PARTY_NOTICES.md` with the version, commit, npm integrity, and any
   bundled dependency whose payload omits its license notice.
5. From the repo root, run `npm install`, `npm run catalog:generate`, and
   `npm run check`.
6. Inspect `npm pack --dry-run --json --workspace plugins/ask-user` and publish a
   new wrapper version before publishing the Pi Kit catalog update.
