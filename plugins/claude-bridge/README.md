# @yassimba/pi-claude-bridge

A reviewed distribution of [Pi Claude Bridge](https://github.com/elidickinson/pi-claude-bridge)
for the Yassimba setup catalog. This package contains the exact upstream npm
release recorded in [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md). It adds no
runtime code of its own.

## Install

Select **Claude Bridge** with `ai-setup add`, or install it directly:

```sh
pi install npm:@yassimba/pi-claude-bridge
```

Restart Pi or run `/reload`. Use `/model` to select a `claude-bridge` model. When
another provider is active, the package also supplies the `AskClaude` tool for
delegating analysis or implementation to Claude Code.

Claude Bridge launches Claude Code through Anthropic's Agent SDK and uses your
Claude Code authentication. Install and authenticate Claude Code before using it.
See the [upstream README](https://github.com/elidickinson/pi-claude-bridge#readme)
for available models, context-window eligibility, configuration, billing notes,
and debugging instructions.

Pi extensions and Claude Code run with your user account's permissions. The
`AskClaude` tool defaults to read-only mode, but its `full` mode can edit files
and execute commands without Pi mediating each action. Review the upstream source
and configuration before enabling it.

## Updating the bundled release

1. Inspect the new upstream npm tarball, source commit, dependencies, and licenses.
2. Set an exact `pi-claude-bridge` version in `package.json`.
3. From `plugins/claude-bridge`, run
   `npm install --package-lock-only --ignore-scripts --omit=peer --install-strategy=nested --workspaces=false`
   to update `npm-shrinkwrap.json`.
4. Update `THIRD_PARTY_NOTICES.md` with the version, commit, npm integrity, and any
   bundled dependency whose payload omits its license notice.
5. From the repo root, run `npm install`, `npm run catalog:generate`, and
   `npm run check`.
6. Inspect `npm pack --dry-run --json --workspace plugins/claude-bridge` and publish
   a new wrapper version before publishing the Pi Kit catalog update.
