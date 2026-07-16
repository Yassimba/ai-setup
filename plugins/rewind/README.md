# @yassimba/pi-rewind

A reviewed distribution of [Pi Rewind](https://github.com/nicobailon/pi-rewind-hook)
for the Yassimba setup catalog. This package contains the exact upstream npm
release recorded in [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md). It adds no
runtime code of its own.

## Install

Select **Rewind** with `ai-setup add`, or install it directly:

```sh
pi install npm:@yassimba/pi-rewind
```

Restart Pi or run `/reload`. Requires Pi v0.65+ and a git repository.

The extension records exact file-state checkpoints as you work. When you branch
to an earlier message with Pi's `/tree` navigation, it offers
"Restore files to that point" so the working tree matches the conversation.
Checkpoint metadata lives inside the session, so history survives forks,
resumes, tree navigation, and compaction. Snapshots stay reachable through a
single git ref; retention is optional and configurable.

Pi extensions run with your user account's permissions. Review the upstream
source before enabling.

## Updating the bundled release

1. Inspect the new upstream npm tarball, source commit, and license.
2. Set an exact `pi-rewind-hook` version in `package.json`.
3. From `plugins/rewind`, run
   `npm install --package-lock-only --ignore-scripts --omit=peer --install-strategy=nested --workspaces=false`
   to update `npm-shrinkwrap.json`.
4. Update `THIRD_PARTY_NOTICES.md` with the version, commit, and npm integrity.
5. From the repo root, run `npm install`, `npm run catalog:generate`, and
   `npm run check`.
6. Inspect `npm pack --dry-run --json --workspace plugins/rewind` and publish a
   new wrapper version before publishing the Pi Kit catalog update.
