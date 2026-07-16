# Releasing

How to cut a herdr-reviewr release from the skills monorepo. A `herdr-reviewr-v*` tag push is
the trigger: the repo-root `.github/workflows/herdr-reviewr-release.yml` creates the GitHub
Release and uploads a prebuilt binary per target (macOS, Linux, and Windows), and
`herdr/install.sh` (unix) / `herdr/install.ps1` (Windows) download the matching asset on
`herdr plugin install`.

## The one rule

**The manifest version and the tag must match.** The install scripts read `version` from
`herdr-plugin.toml`, set the tag to `herdr-reviewr-v<version>`, and download from
`releases/download/<tag>/`. A `0.2.0` manifest needs a `herdr-reviewr-v0.2.0` tag, or
installs 404. Tags are per-plugin because this repo hosts more than one herdr plugin.

Two files carry the version — keep them equal:

- `Cargo.toml` → `[package] version`
- `herdr-plugin.toml` → `version`

## Steps

Pick the new version with semver: a behavior change or new feature is a minor bump in `0.x`
(`0.1.1 → 0.2.0`); a fix-only release is a patch (`0.2.0 → 0.2.1`).

1. **Bump both versions** to the new `X.Y.Z` — `Cargo.toml` and `herdr-plugin.toml`.
2. **Finalize the changelog** — rename `## [Unreleased]` to `## [X.Y.Z] — <date>` and add a fresh
   empty `## [Unreleased]` above it. The format is [Keep a Changelog](https://keepachangelog.com).
3. **Refresh the lock** — `cargo build` so `Cargo.lock`'s `herdr-reviewr` entry updates to `X.Y.Z`.
4. **Verify green** — `just ci` (fmt-check, clippy, test, release build).
5. **Commit** the bump + changelog on a branch, review, and land it on `main`.
6. **Tag and push** — an annotated tag whose name is `herdr-reviewr-vX.Y.Z`:

   ```bash
   git checkout main && git pull
   git tag -a herdr-reviewr-vX.Y.Z -m "herdr-reviewr vX.Y.Z"
   git push origin main
   git push origin herdr-reviewr-vX.Y.Z   # triggers herdr-reviewr-release.yml
   ```

7. **Watch the build** and confirm the assets landed:

   ```bash
   gh run watch                              # the herdr-reviewr-release.yml run for the tag
   gh release view herdr-reviewr-vX.Y.Z      # six <target>.tar.gz + .sha256 sidecars
   ```

## What the tag triggers

`herdr-reviewr-release.yml` (on `push: tags: ["herdr-reviewr-v*"]`):

- creates the Release for the tag if absent (`gh release create --verify-tag --generate-notes`);
- builds `herdr-reviewr` for `aarch64-apple-darwin`, `x86_64-apple-darwin`,
  `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`, and
  `aarch64-pc-windows-msvc`;
- uploads each as `herdr-reviewr-<target>.tar.gz` with a `.sha256` sidecar.

The toolchain is pinned by `rust-toolchain.toml`, so CI and local builds match.

## Reinstall locally after a release

Switch your own machine from the dev link to the published release. This is also the cheapest
end-to-end test: it exercises the exact `herdr plugin install` path a user hits.

1. **Swap the link for the release.** Your config survives — `config.toml` lives in
   `~/.config/herdr/plugins/config/yassimba.reviewr/`, keyed by plugin id, untouched by a reinstall.

   ```bash
   herdr plugin unlink yassimba.reviewr
   herdr plugin install Yassimba/ai-setup/plugins/herdr-reviewr --yes   # downloads the vX.Y.Z binary
   herdr plugin list --plugin yassimba.reviewr                          # confirm: github source + version X.Y.Z
   ```

2. **Relaunch the sidebar** so the open pane runs the new binary instead of the old process.
   The `close` and `open` actions own the pane lifecycle — there is no state file to sync:

   ```bash
   herdr plugin action invoke close --plugin yassimba.reviewr   # closes every reviewr pane
   herdr plugin action invoke open  --plugin yassimba.reviewr   # opens the new binary
   ```

**Gotchas**

- The actions act on the focused workspace. Focus the workspace you want relaunched first.
- Closing then immediately reopening can briefly leave two `reviewr` panes (async lag) — a
  single `close` sweeps them all.

## Notes

- **`min_herdr_version`** (in `herdr-plugin.toml`) only changes when a release depends on a newer
  herdr API. A normal feature release leaves it as is.
- **Code signing** is a local-dev concern, not a release one: CI produces fresh binaries, while a
  contributor's in-place rebuild needs `just install` (see the README) to avoid an Apple-Silicon
  SIGKILL. Release assets are downloaded fresh by `install.sh`, so they are unaffected.
- **`--verify-tag`** means the tag must exist on the remote before the Release is created — push
  the tag, don't create the Release by hand first.
