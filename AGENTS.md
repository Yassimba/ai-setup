# skills

A public hub for my favorite agent skills to share with people. Install with `npx skills add` (Vercel)
or through the Claude plugin marketplace.

## Layout

`skills/<name>/SKILL.md` — the shared skills, one flat directory per skill.
The category grouping shown by skills.sh lives in `skills.sh.json`, not in
folders. The Claude plugin is the repo root itself: `.claude-plugin/plugin.json`
plus auto-discovered `skills/`, listed by `.claude-plugin/marketplace.json`
(`source: "./"`).
`plugins/<name>/` — one flat directory per plugin package: npm workspaces
(Pi extensions, discovered by their `package.json`) and Rust Herdr plugins
(installable via `herdr plugin install Yassimba/ai-setup/plugins/<name>`)
side by side.
`cli/ai-setup/` — the Rust onboarding CLI installed by the root bootstrap
scripts. `setup-catalog.json` is generated from reviewed skills and opted-in
plugin metadata; the CLI embeds it, copies skills into the agent skill trees
itself, and delegates Pi and Herdr installs to their own managers.
`drafts/` — pulled from a global agent tree, not yet reviewed.
`personal/` — machine-specific skills, not for anyone else. Kept outside
`skills/` so no installer surface (npx, Claude plugin, the CLI) can ever see
them.

## Sync

The repo is the only real copy of each skill. `scripts/sync-skills.sh link`
symlinks `skills/<name>` and `personal/<name>` into the global agent trees
(`~/.claude/skills`, `~/.agents/skills`, `~/.codex/skills`, and
`~/.pi/agent/skills`), so an edit made from either side is the same file and
syncing means committing. `pull` brings
a diverged or unknown global skill back into the repo (unknown ones land in
`drafts/`), `status` shows the picture, `unlink` materializes a real copy for
testing what installers ship. No command destroys information: link only
replaces dirs proven identical, pull requires a clean git path.

Pi extensions follow the same model. `scripts/sync-pi-extensions.sh link`
discovers the concrete `pi.extensions` entrypoints in `plugins/*/package.json`
and symlinks them into `~/.pi/agent/extensions`. Use `status`, `pull`, and
`unlink` with the same meanings and safety guarantees as the skill sync.
The script skips full Pi packages (ones that also declare `pi.skills` or
`pi.prompts`): those only load correctly through `pi install <path>` package
installs, and a bare extension symlink would drop their skills/prompts and
double-load the extension.

`plugins/pi-subagents` is a vendored fork of
[nicobailon/pi-subagents](https://github.com/nicobailon/pi-subagents) (MIT,
upstream author credited in its package.json). The git bridge to upstream is
the standalone clone at `~/Documents/projects/pi-subagents` (`origin` =
Yassimba/pi-subagents fork, `upstream` = nicobailon): pull upstream there,
then rsync into `plugins/pi-subagents` (minus `.git`, `node_modules`,
`package-lock.json`) and commit here. Its 39k third-party lines are excluded
from repo gates (biome, tsc, fallow) — run its own `npm test` inside the
package instead.

`plugins/herdr-reviewr` is developed here as `yassimba.reviewr`. It started
from [persiyanov/herdr-reviewr](https://github.com/persiyanov/herdr-reviewr)
(MIT, vendored at v0.11.0 via `git subtree add --squash`) but has diverged far
past routine subtree pulls — GitLab MR support, Windows builds, remote diff
review, the built-in project switcher. Treat it as first-party: don't
`git subtree pull` from upstream; if an upstream change is worth having,
port it by hand and note the origin in the commit.
It releases from this repo through the generated `release/auto` PR. Merging that
PR creates a `herdr-reviewr-v<version>` tag and the shared release workflow
cross-compiles 6 targets, then attaches the binaries its installers download.
Its Rust tree is excluded from the JS repo gates (fallow ignore, biome
allowlist); the inferred Rust CI matrix runs its format, Clippy, test, and build
commands when its path changes.
See `ai-docs/adr/0001-vendor-herdr-plugins-via-subtree.md`.

`plugins/mindwalk` is a squashed subtree of
[cosmtrek/mindwalk](https://github.com/cosmtrek/mindwalk) plus ZacharyQin's Pi adapter
from upstream PR #4. It is released here as `@yassimba/pi-mindwalk`; its npm prepack
cross-compiles the Go application for all supported platforms and bundles those binaries,
so installation needs neither Go nor a postinstall download. Pull upstream deliberately
with `git subtree pull --prefix plugins/mindwalk ... --squash`, then port the Pi package
changes across any conflicts and preserve `THIRD_PARTY_NOTICES.md`.

Note: the global `merge` skill is not a rename of `resolving-merge-conflicts`
— it performs a branch merge rather than resolving a conflict in progress.

## Releases

Use Conventional Commits. After a main-branch change, `release-pr.yml` discovers
which npm, Cargo, and Herdr components changed and opens or updates
`release/auto`. Breaking changes bump major, features bump minor, and fixes or
other product changes bump patch; README, changelog, tests, and CI-only changes
do not release. Merging that PR publishes the recorded `.release-plan.json` only
after consolidated CI passes. Do not create component tags by hand.

## Quality checks

After finishing code changes (plugins/, scripts/, test/), run
`npm run audit` — it gates on dead code, complexity, and duplication that the
changeset introduced (config in `.fallowrc.json`; `npm run audit:brief` gives
a non-gating orientation report). `npm run check` covers the rest: tests,
tsc, biome lint, catalog, and packaging.

## Rule

Everything under `skills/` is public: npx, the Claude plugin, and the CLI all
discover the flat tree directly. `skills.sh.json` lists shared skills only —
it is the grouping shown by skills.sh, nothing more.

Never move anything from `drafts/` or `personal/` into `skills/` or list it in
`skills.sh.json` without review. All are public install surfaces; drafts are
unreviewed and personal skills hardcode local paths. Promote a draft into
`skills/` first (`sync-skills.sh promote <name>`), then list it.

A skill whose flow invokes another skill (`Run a /grilling session`, `route
through /diagnosing-bugs`) declares it in a per-skill `deps.yml` next to its
SKILL.md, as a bare skill name, so installers pull deps in transitively (the
catalog generator bakes the graph into `setup-catalog.json` for the CLI).
Declare invocations only — not soft "if installed" mentions or pointers.
`sync-skills.sh deps` validates the sidecars: every dep must be a shared skill
at its stated path and the graph must stay acyclic. On this machine the agent
trees are symlinked into the repo (`sync-skills.sh link`); `ai-setup update`
never writes through those symlinks, so keep using `pull` before `link` for
divergences.
