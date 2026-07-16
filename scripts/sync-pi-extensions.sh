#!/usr/bin/env bash
set -euo pipefail

# One source of truth: repository-owned Pi extension entrypoints declared by
# plugins/*/package.json. Dependency entrypoints under node_modules are bundled
# release payloads, not editable repo source, so discovery ignores them. The global
# Pi extension tree holds symlinks into source directories, so edits from either
# side change the same versioned files.
#
#   status          show linked, copied, diverged, conflicting, and absent entries
#   link   [name]   repo -> ~/.pi/agent/extensions as symlinks
#   pull   [name]   global real copy -> repo (requires a clean git path)
#   unlink [name]   materialize managed symlinks as real copies for testing
#
# link only replaces a real path after proving it byte-for-byte identical.
# Divergent copies, wrong symlinks, and legacy single-file conflicts are never
# overwritten. Run `status` to see the logical names discovered from Pi package
# manifests (for example: ask-user, herdr-worktree, openai-fast).

REPO="${PI_EXTENSIONS_REPO:-$(cd "$(dirname "$0")/.." && pwd)}"
PACKAGES="$REPO/plugins"
AGENT_DIR="${PI_CODING_AGENT_DIR:-$HOME/.pi/agent}"
GLOBAL="$AGENT_DIR/extensions"

say()  { printf '%s\n' "$*"; }
item() { printf '  %s\n' "$*"; }
die()  { printf 'error: %s\n' "$*" >&2; exit 1; }

command -v node >/dev/null || die "node not found"
command -v rsync >/dev/null || die "rsync not found"
[ -d "$PACKAGES" ] || die "plugins directory not found: $PACKAGES"

# Emits: logical-name<TAB>source-path<TAB>global-leaf<TAB>entrypoint-file
#
# An index.ts-style entrypoint links its containing directory. A package with a
# single entrypoint uses the workspace directory name. A package with multiple
# entrypoints uses each entrypoint's parent directory name.
ENTRIES="$(node --input-type=module - "$PACKAGES" <<'NODE'
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { basename, dirname, extname, join, resolve } from "node:path";

const packagesRoot = process.argv[2];
const rows = [];
const names = new Set();
for (const workspace of readdirSync(packagesRoot, { withFileTypes: true })) {
  if (!workspace.isDirectory()) continue;
  const packageDirectory = join(packagesRoot, workspace.name);
  const manifestPath = join(packageDirectory, "package.json");
  if (!existsSync(manifestPath)) continue;
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  const entrypoints = manifest?.pi?.extensions;
  if (!Array.isArray(entrypoints)) continue;
  // Full Pi packages also register skills/prompts, which only load through
  // `pi install <path>` package installs; a bare extension symlink would drop
  // them and double-load the extension. Leave those to the package mechanism.
  if (manifest?.pi?.skills || manifest?.pi?.prompts) continue;
  const sourceEntrypoints = entrypoints.filter(
    (configuredPath) =>
      typeof configuredPath !== "string" ||
      !/(^|[\\/])node_modules([\\/]|$)/.test(configuredPath),
  );
  for (const configuredPath of sourceEntrypoints) {
    if (typeof configuredPath !== "string" || /[*?!{}[\]]/.test(configuredPath)) {
      throw new Error(`${manifestPath}: sync requires concrete pi.extensions paths`);
    }
    const entrypoint = resolve(packageDirectory, configuredPath);
    if (!existsSync(entrypoint)) throw new Error(`missing extension entrypoint: ${entrypoint}`);
    const extension = extname(entrypoint);
    const isIndex = /^index\.(?:[cm]?[jt]s)$/i.test(basename(entrypoint));
    const source = isIndex ? dirname(entrypoint) : entrypoint;
    const logicalName =
      sourceEntrypoints.length === 1
        ? workspace.name
        : isIndex
          ? basename(dirname(entrypoint))
          : `${workspace.name}-${basename(entrypoint, extension)}`;
    if (names.has(logicalName)) throw new Error(`duplicate extension sync name: ${logicalName}`);
    names.add(logicalName);
    const globalLeaf = isIndex ? logicalName : `${logicalName}${extension}`;
    rows.push([logicalName, source, globalLeaf, entrypoint]);
  }
}
rows.sort((left, right) => left[0].localeCompare(right[0]));
for (const row of rows) process.stdout.write(`${row.join("\t")}\n`);
NODE
)" || die "failed to discover Pi extension entrypoints"

[ -n "$ENTRIES" ] || die "no Pi extension entrypoints found under plugins/"

repo_entries() { printf '%s\n' "$ENTRIES"; }
repo_names() { repo_entries | cut -f1; }

entry_for() {
  local wanted="$1" name source leaf entrypoint
  while IFS=$'\t' read -r name source leaf entrypoint; do
    if [ "$name" = "$wanted" ]; then
      printf '%s\t%s\t%s\t%s\n' "$name" "$source" "$leaf" "$entrypoint"
      return 0
    fi
  done < <(repo_entries)
  return 1
}

same_path() {
  local left="$1" right="$2"
  if [ -d "$left" ] && [ -d "$right" ]; then
    diff -rq --exclude='.DS_Store' "$left" "$right" >/dev/null 2>&1
  elif [ -f "$left" ] && [ -f "$right" ]; then
    cmp -s "$left" "$right"
  else
    return 1
  fi
}

clean_or_die() {
  local absolute="$1" relative
  git -C "$REPO" rev-parse --is-inside-work-tree >/dev/null 2>&1 \
    || die "repo is not a git worktree: $REPO"
  relative="${absolute#"$REPO"/}"
  [ "$relative" != "$absolute" ] || die "refusing to pull outside repo: $absolute"
  [ -z "$(git -C "$REPO" status --porcelain -- "$relative" 2>/dev/null)" ] \
    || die "uncommitted changes at $relative — commit them before pulling over them"
}

copy_path() {
  local source="$1" target="$2"
  if [ -d "$source" ]; then
    mkdir -p "$target"
    rsync -a --delete --exclude '.DS_Store' "$source/" "$target/"
  else
    mkdir -p "$(dirname "$target")"
    cp "$source" "$target"
  fi
}

legacy_paths() {
  local name="$1" candidate
  for candidate in "$GLOBAL/$name.ts" "$GLOBAL/$name.js" "$GLOBAL/$name.mts" \
    "$GLOBAL/$name.mjs" "$GLOBAL/$name.cts" "$GLOBAL/$name.cjs"; do
    if [ -e "$candidate" ] || [ -L "$candidate" ]; then printf '%s\n' "$candidate"; fi
  done
}

legacy_state() {
  local name="$1" entrypoint="$2" count=0 candidate only=""
  while IFS= read -r candidate; do
    [ -n "$candidate" ] || continue
    count=$((count + 1))
    only="$candidate"
  done < <(legacy_paths "$name")
  if [ "$count" -eq 0 ]; then printf 'none\t\n'; return; fi
  if [ "$count" -gt 1 ]; then printf 'multiple\t\n'; return; fi
  if [ -L "$only" ]; then printf 'symlink\t%s\n' "$only"; return; fi
  if same_path "$entrypoint" "$only"; then
    printf 'identical\t%s\n' "$only"
  else
    printf 'diverged\t%s\n' "$only"
  fi
}

link_one() {
  local name="$1" row source leaf entrypoint target legacy_kind legacy
  row="$(entry_for "$name")" || die "no Pi extension named '$name'"
  IFS=$'\t' read -r _ source leaf entrypoint <<<"$row"
  target="$GLOBAL/$leaf"

  if [ -L "$target" ]; then
    if [ "$(readlink "$target")" = "$source" ]; then
      item "linked   $name"
    else
      # A link into this repo whose target moved carries no data of its own —
      # re-point it. Anything else is someone else's link: leave it.
      case "$(readlink "$target")" in
        "$REPO"/*)
          rm "$target"
          ln -s "$source" "$target"
          item "repoint  $name -> $source"
          ;;
        *) item "conflict $name  ($target links to $(readlink "$target"))" ;;
      esac
    fi
    return
  fi
  if [ -e "$target" ]; then
    if same_path "$source" "$target"; then
      rm -rf "$target"
      ln -s "$source" "$target"
      item "relink   $name  (identical copy replaced by a link)"
    else
      item "diverged $name  — run: scripts/sync-pi-extensions.sh pull $name"
    fi
    return
  fi

  IFS=$'\t' read -r legacy_kind legacy <<<"$(legacy_state "$name" "$entrypoint")"
  case "$legacy_kind" in
    none) ;;
    identical)
      rm "$legacy"
      item "migrate  $name  (identical legacy file removed)"
      ;;
    diverged)
      item "conflict $name  ($legacy differs; pull or move it first)"
      return
      ;;
    *)
      item "conflict $name  (multiple or symlinked legacy files exist)"
      return
      ;;
  esac

  ln -s "$source" "$target"
  item "link     $name -> $target"
}

pull_one() {
  local name="$1" row source leaf entrypoint target legacy_kind legacy
  row="$(entry_for "$name")" || die "no Pi extension named '$name'"
  IFS=$'\t' read -r _ source leaf entrypoint <<<"$row"
  target="$GLOBAL/$leaf"

  if [ -L "$target" ]; then
    [ "$(readlink "$target")" = "$source" ] \
      && item "linked   $name  (already the repo source)" \
      || item "conflict $name  ($target links elsewhere)"
    return
  fi
  if [ -e "$target" ]; then
    if same_path "$source" "$target"; then
      item "same     $name"
      return
    fi
    clean_or_die "$source"
    copy_path "$target" "$source"
    item "pulled   $name -> ${source#"$REPO"/}  (review with git diff)"
    return
  fi

  IFS=$'\t' read -r legacy_kind legacy <<<"$(legacy_state "$name" "$entrypoint")"
  case "$legacy_kind" in
    identical) item "same     $name  (legacy file matches)" ;;
    diverged)
      clean_or_die "$entrypoint"
      cp "$legacy" "$entrypoint"
      item "pulled   $name -> ${entrypoint#"$REPO"/}  (from legacy file; review with git diff)"
      ;;
    none) item "absent   $name" ;;
    *) item "conflict $name  (cannot choose a legacy source)" ;;
  esac
}

unlink_one() {
  local name="$1" row source leaf entrypoint target
  row="$(entry_for "$name")" || die "no Pi extension named '$name'"
  IFS=$'\t' read -r _ source leaf entrypoint <<<"$row"
  target="$GLOBAL/$leaf"
  if [ ! -L "$target" ]; then
    item "skip     $name  (not a managed symlink)"
    return
  fi
  if [ "$(readlink "$target")" != "$source" ]; then
    item "conflict $name  ($target links elsewhere)"
    return
  fi
  rm "$target"
  copy_path "$source" "$target"
  item "copied   $name  ($target is now a real copy)"
}

state_one() {
  local name="$1" row source leaf entrypoint target legacy_kind legacy
  row="$(entry_for "$name")" || return 1
  IFS=$'\t' read -r _ source leaf entrypoint <<<"$row"
  target="$GLOBAL/$leaf"
  if [ -L "$target" ]; then
    [ "$(readlink "$target")" = "$source" ] && printf 'linked' || printf 'conflict'
  elif [ -e "$target" ]; then
    same_path "$source" "$target" && printf 'copy' || printf 'diverged'
  else
    IFS=$'\t' read -r legacy_kind legacy <<<"$(legacy_state "$name" "$entrypoint")"
    case "$legacy_kind" in
      none) printf 'absent' ;;
      identical) printf 'legacy-copy' ;;
      diverged) printf 'legacy-diverged' ;;
      *) printf 'conflict' ;;
    esac
  fi
}

is_managed_global_leaf() {
  local wanted="$1" name source leaf entrypoint extension
  while IFS=$'\t' read -r name source leaf entrypoint; do
    [ "$wanted" = "$leaf" ] && return 0
    if [ -d "$source" ]; then
      for extension in ts js mts mjs cts cjs; do
        [ "$wanted" = "$name.$extension" ] && return 0
      done
    fi
  done < <(repo_entries)
  return 1
}

cmd_status() {
  local name state count=0 path leaf unmanaged=0
  say "repo: $(repo_names | wc -l | tr -d ' ') Pi extension entrypoints"
  say "global: $GLOBAL"
  say ""
  while IFS= read -r name; do
    [ -n "$name" ] || continue
    state="$(state_one "$name")"
    case "$state" in
      linked)          item "= $name  linked" ;;
      copy)            item "+ $name  identical copy" ;;
      diverged)        item "~ $name  diverged copy" ;;
      legacy-copy)     item "+ $name  identical legacy file" ;;
      legacy-diverged) item "! $name  diverged legacy file" ;;
      conflict)        item "! $name  conflicting path or symlink" ;;
      absent)          item "- $name  absent" ;;
    esac
    count=$((count + 1))
  done < <(repo_names)
  [ "$count" -gt 0 ] || item "(none)"

  say ""
  say "global-only (left untouched):"
  for path in "$GLOBAL"/*; do
    if [ ! -e "$path" ] && [ ! -L "$path" ]; then continue; fi
    leaf="$(basename "$path")"
    is_managed_global_leaf "$leaf" && continue
    item "? $leaf"
    unmanaged=$((unmanaged + 1))
  done
  [ "$unmanaged" -gt 0 ] || item "(none)"
}

run_for_entries() {
  local operation="$1" only="${2:-}" name
  if [ -n "$only" ]; then
    entry_for "$only" >/dev/null || die "no Pi extension named '$only'"
    "$operation" "$only"
    return
  fi
  while IFS= read -r name; do
    [ -n "$name" ] && "$operation" "$name"
  done < <(repo_names)
}

main() {
  local command="${1:-status}" only="${2:-}"
  [ "$#" -le 2 ] || die "usage: $0 {status|link|pull|unlink} [name]"
  mkdir -p "$GLOBAL"
  case "$command" in
    status) [ -z "$only" ] || die "status does not take a name"; cmd_status ;;
    link)   run_for_entries link_one "$only" ;;
    pull)   run_for_entries pull_one "$only" ;;
    unlink) run_for_entries unlink_one "$only" ;;
    *) die "usage: $0 {status|link|pull|unlink} [name]" ;;
  esac
}

main "$@"
