#!/usr/bin/env bash
set -euo pipefail

# One source of truth: skills/<name> (shared) and personal/<name>
# (machine-specific) in this repo. The global agent trees hold symlinks into
# it, so an edit — made from either side — is the same file, versioned by git.
# There is no copy-sync, no backup attic, no dry-run flag: every command
# either creates links, writes into the git worktree, or refuses.
#
#   status                   what is linked, diverged, unknown, or absent
#   link    [name]           repo -> trees, as symlinks. Replaces a real dir
#                            only after proving it identical to the repo copy;
#                            a diverged dir is left alone with a pull hint.
#   pull    [name]           ~/.claude/skills -> repo. Copies a diverged global
#                            dir over the repo copy (review with git diff);
#                            copies an unknown skill into drafts/. Refuses to
#                            overwrite uncommitted repo files.
#   unlink  [name]           turn a symlink back into a real copy — for testing
#                            what `npx skills add` users get.
#   promote <name> [personal]  drafts/<name> -> skills/<name> (or
#                            personal/<name>), then link it everywhere.
#   deps                     validate the per-skill deps.yml sidecars: every
#                            dep must be a shared skill at its stated path and
#                            the graph must stay acyclic. Installers pull
#                            declared deps in transitively; nothing is generated.
#
# The invariant: no command destroys information. link only removes dirs it
# verified identical, pull lands in a clean git worktree, unlink materializes
# a copy in place of a link.

REPO="$(cd "$(dirname "$0")/.." && pwd)"
SKILLS="$REPO/skills"
PERSONAL="$REPO/personal"
DRAFTS="$REPO/drafts"

# pull reads from here — the tree agents actually edit.
PRIMARY="$HOME/.claude/skills"

# link/unlink/status touch all of these. Narrow with --tree claude|agents|codex|pi.
TREES=("$HOME/.claude/skills" "$HOME/.agents/skills" "$HOME/.codex/skills" "$HOME/.pi/agent/skills")

say()  { printf '%s\n' "$*"; }
item() { printf '  %s\n' "$*"; }
die()  { printf 'error: %s\n' "$*" >&2; exit 1; }

command -v rsync >/dev/null || die "rsync not found"

# A skill is a real directory holding a SKILL.md — never a symlink.
is_skill() { [ -d "$1" ] && [ ! -L "$1" ] && [ -f "$1/SKILL.md" ]; }

same() { diff -rq --exclude='.DS_Store' "$1" "$2" >/dev/null 2>&1; }

# repo_path <name> -> skills/<name> or personal/<name>, or empty. Both roots
# are flat; skills.sh.json carries the category grouping for skills.sh.
repo_path() {
  local name="$1" root
  for root in "$SKILLS" "$PERSONAL"; do
    if is_skill "$root/$name"; then printf '%s' "$root/$name"; return; fi
  done
}

repo_names() {
  local root skill
  for root in "$SKILLS" "$PERSONAL"; do
    [ -d "$root" ] || continue
    for skill in "$root"/*/; do
      [ -d "$skill" ] || continue
      is_skill "${skill%/}" && basename "${skill%/}"
    done
  done | sort
}

# Real skill dirs in an agent tree — symlinks and dotdirs excluded.
tree_names() {
  local dir="$1" skill
  [ -d "$dir" ] || return 0
  for skill in "$dir"/*/; do
    [ -d "$skill" ] || continue
    is_skill "${skill%/}" && basename "${skill%/}"
  done | sort
}

# Overwriting an uncommitted path would lose the only copy of whatever was
# there. Untracked counts: git checkout cannot bring those files back either.
clean_or_die() {
  local rel="$1"
  [ -z "$(git -C "$REPO" status --porcelain -- "$rel" 2>/dev/null)" ] \
    || die "uncommitted changes at $rel — commit them before pulling over them"
}

copy_skill() { rsync -a --delete --exclude '.DS_Store' "$1/" "$2/"; }

link_one() {
  local name="$1" tree="$2" src target
  src="$(repo_path "$name")"
  target="$tree/$name"

  if [ -L "$target" ]; then
    [ "$(readlink "$target")" = "$src" ] && return 0
    # A link into this repo whose target moved (e.g. the old categorized
    # layout) carries no data of its own — re-point it. Anything else is
    # someone else's link: leave it.
    case "$(readlink "$target")" in
      "$REPO"/*)
        rm "$target"
        ln -s "$src" "$target"
        item "repoint  $name -> $src"
        ;;
      *) item "skip     $name  ($target already links to $(readlink "$target"))" ;;
    esac
    return 0
  fi
  if [ -e "$target" ]; then
    if same "$src" "$target"; then
      rm -rf "$target"
      ln -s "$src" "$target"
      item "relink   $name  (identical copy replaced by a link)"
    else
      item "diverged $name  — run: scripts/sync-skills.sh pull $name, then link again"
    fi
    return 0
  fi
  ln -s "$src" "$target"
  item "link     $name -> $target"
}

cmd_link() {
  local only="${1:-}" tree n
  if [ -n "$only" ]; then
    [ -n "$(repo_path "$only")" ] || die "no skill named '$only' under skills/"
  fi
  for tree in "${TREES[@]}"; do
    mkdir -p "$tree"
    say "$tree:"
    while read -r n; do
      [ -n "$n" ] || continue
      [ -n "$only" ] && [ "$n" != "$only" ] && continue
      link_one "$n" "$tree"
    done < <(repo_names)
  done
}

cmd_pull() {
  local only="${1:-}" n p pulled=0 drafted=0
  while read -r n; do
    [ -n "$n" ] || continue
    [ -n "$only" ] && [ "$n" != "$only" ] && continue
    p="$(repo_path "$n")"

    if [ -n "$p" ]; then
      same "$PRIMARY/$n" "$p" && continue
      clean_or_die "${p#"$REPO"/}"
      copy_skill "$PRIMARY/$n" "$p"
      item "pulled   $n -> ${p#"$REPO"/}  (review with git diff)"
      pulled=$((pulled + 1))
    else
      [ -d "$DRAFTS/$n" ] && same "$PRIMARY/$n" "$DRAFTS/$n" && continue
      clean_or_die "drafts/$n"
      mkdir -p "$DRAFTS"
      copy_skill "$PRIMARY/$n" "$DRAFTS/$n"
      item "drafted  $n -> drafts/$n"
      drafted=$((drafted + 1))
    fi
  done < <(tree_names "$PRIMARY")

  say ""
  say "$pulled pulled into skills/, $drafted copied to drafts/"
  [ "$drafted" -gt 0 ] && say "drafts are inert until promoted: scripts/sync-skills.sh promote <name> [personal]"
  return 0
}

cmd_unlink() {
  local only="${1:-}" tree n target src
  for tree in "${TREES[@]}"; do
    [ -d "$tree" ] || continue
    for target in "$tree"/*/; do
      target="${target%/}"
      n="$(basename "$target")"
      [ -n "$only" ] && [ "$n" != "$only" ] && continue
      [ -L "$target" ] || continue
      src="$(readlink "$target")"
      case "$src" in "$SKILLS"/*|"$PERSONAL"/*) ;; *) continue ;; esac
      rm "$target"
      cp -R "$src" "$target"
      item "copied   $n  ($target is now the real copy installers would ship)"
    done
  done
}

cmd_status() {
  local tree n p target linked copies diverged absent
  say "repo: $(repo_names | wc -l | tr -d ' ') skills under skills/ + personal/, $(tree_names "$DRAFTS" | wc -l | tr -d ' ') drafts"
  say ""
  for tree in "${TREES[@]}"; do
    linked=0; copies=0; diverged=0; absent=0
    while read -r n; do
      [ -n "$n" ] || continue
      p="$(repo_path "$n")"
      target="$tree/$n"
      if [ -L "$target" ] && [ "$(readlink "$target")" = "$p" ]; then
        linked=$((linked + 1))
      elif is_skill "$target"; then
        if same "$p" "$target"; then copies=$((copies + 1)); else diverged=$((diverged + 1)); fi
      elif [ ! -e "$target" ]; then
        absent=$((absent + 1))
      fi
    done < <(repo_names)
    say "$tree: $linked linked, $copies identical copies, $diverged diverged, $absent absent"
  done

  local any
  say ""; say "diverged in $PRIMARY (pull, review the git diff, then link):"
  any=false
  while read -r n; do
    [ -n "$n" ] || continue
    p="$(repo_path "$n")"
    if is_skill "$PRIMARY/$n" && ! same "$p" "$PRIMARY/$n"; then item "~ $n"; any=true; fi
  done < <(repo_names)
  $any || item "(none)"

  say ""; say "global-only in $PRIMARY (pull copies them to drafts/, or ignore):"
  any=false
  while read -r n; do
    [ -n "$n" ] || continue
    [ -z "$(repo_path "$n")" ] || continue
    if [ -d "$DRAFTS/$n" ]; then item "? $n  (already drafted)"; else item "? $n"; fi
    any=true
  done < <(tree_names "$PRIMARY")
  $any || item "(none)"
}

cmd_deps() {
  command -v python3 >/dev/null || die "python3 not found (needed to read skills.sh.json)"
  # Per-skill deps.yml sidecars declare the skills a skill invokes, so
  # installers pull them in transitively. Every declared dep must itself be a
  # shared skill at its stated path, and the graph must stay acyclic.
  python3 - "$REPO" <<'PYEOF' || die "deps.yml dependency validation failed"
import json, re, sys
from pathlib import Path
repo = Path(sys.argv[1])
shared = {s for g in json.loads((repo/"skills.sh.json").read_text())["groupings"] for s in g["skills"]}
graph, bad = {}, []
for f in repo.glob("skills/*/deps.yml"):
    skill = f.parent.name
    deps = re.findall(r"^\s*-\s+([\w-]+)\s*$", f.read_text(), re.M)
    graph[skill] = deps
    if not deps:
        bad.append(f"{f.relative_to(repo)}: declares no deps — delete the file instead")
    for name in deps:
        if name not in shared:
            bad.append(f"{f.relative_to(repo)}: dep '{name}' is not a shared skill")
        if not (repo/"skills"/name/"SKILL.md").exists():
            bad.append(f"{f.relative_to(repo)}: dep path skills/{name} does not exist")
seen, stack = set(), []
def visit(n):
    if n in stack:
        bad.append("dependency cycle: " + " -> ".join(stack[stack.index(n):] + [n]))
        return
    if n in seen: return
    seen.add(n); stack.append(n)
    for m in graph.get(n, []): visit(m)
    stack.pop()
for n in list(graph): visit(n)
for b in bad: print("  invalid  " + b)
if not bad: print(f"  ok       {len(graph)} deps.yml sidecars: deps shared, paths exist, graph acyclic")
sys.exit(1 if bad else 0)
PYEOF
}

cmd_promote() {
  local name="${1:-}" where="${2:-}" dest rel
  [ -n "$name" ] || die "usage: $0 promote <name> [personal]"
  case "$where" in
    "")       dest="$SKILLS/$name";   rel="skills/$name" ;;
    personal) dest="$PERSONAL/$name"; rel="personal/$name" ;;
    *) die "usage: $0 promote <name> [personal]" ;;
  esac
  is_skill "$DRAFTS/$name" || die "no draft skill at drafts/$name"
  [ -z "$(repo_path "$name")" ] || die "'$name' already exists in the repo"

  mkdir -p "$(dirname "$dest")"
  git -C "$REPO" mv "drafts/$name" "$rel" 2>/dev/null \
    || mv "$DRAFTS/$name" "$dest"
  item "promoted $name -> $rel"
  cmd_link "$name"
  say ""
  [ "$where" = personal ] \
    || say "add \"$name\" to a group in skills.sh.json so installers pick it up"
}

main() {
  local cmd="${1:-status}"; shift || true
  local args=() want_tree=""
  while [ $# -gt 0 ]; do
    case "$1" in
      --tree) shift; want_tree="${1:-}"; [ -n "$want_tree" ] || die "--tree needs a value" ;;
      -*) die "unknown flag: $1" ;;
      *) args+=("$1") ;;
    esac
    shift
  done

  if [ -n "$want_tree" ]; then
    case "$want_tree" in
      claude|agents|codex) TREES=("$HOME/.$want_tree/skills") ;;
      pi) TREES=("$HOME/.pi/agent/skills") ;;
      *) die "--tree must be one of: claude agents codex pi" ;;
    esac
  fi

  [ -d "$PRIMARY" ] || die "no skill tree at $PRIMARY"

  case "$cmd" in
    status)  cmd_status ;;
    link)    cmd_link "${args[0]:-}" ;;
    pull)    cmd_pull "${args[0]:-}" ;;
    unlink)  cmd_unlink "${args[0]:-}" ;;
    promote) cmd_promote "${args[@]+"${args[@]}"}" ;;
    deps)    cmd_deps ;;
    *) die "usage: $0 {status|link|pull|unlink|promote|deps} [name] [--tree claude|agents|codex|pi]" ;;
  esac
}

main "$@"
