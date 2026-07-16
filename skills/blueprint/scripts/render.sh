#!/usr/bin/env bash
# render.sh — build the blueprint viewer from Mermaid sources and validate them.
#
# Usage: render.sh [-t "Change name"] [-o out.html] [--open] <diagram.mmd> [more.mmd ...]
#
#   -t      Page title (default: Blueprint). HTML-escaped for you.
#   -o      Output HTML path (default: blueprint.html next to the first .mmd).
#   --open  Open the built page in the default browser.
#
# Each .mmd file becomes one tab; the filename (minus extension, capitalized)
# is the tab label. Sources are HTML-escaped automatically — write plain
# Mermaid, including <<stereotypes>>.
#
# Validation: if a Chrome/Chromium/Edge binary is found, the built page is
# loaded headlessly and every diagram must render. Parse errors are printed
# and the script exits 1. Without a browser (or offline) validation is
# skipped with a warning.
set -euo pipefail

title="Blueprint"
out=""
open_after=0
files=()
while [ $# -gt 0 ]; do
  case "$1" in
    -t) title="$2"; shift 2 ;;
    -o) out="$2"; shift 2 ;;
    --open) open_after=1; shift ;;
    -h|--help) sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) files+=("$1"); shift ;;
  esac
done
if [ ${#files[@]} -eq 0 ]; then
  echo "usage: render.sh [-t title] [-o out.html] [--open] <diagram.mmd>..." >&2
  exit 2
fi
for f in "${files[@]}"; do
  [ -f "$f" ] || { echo "error: no such file: $f" >&2; exit 2; }
done
[ -n "$out" ] || out="$(dirname "${files[0]}")/blueprint.html"

template="$(cd "$(dirname "$0")/.." && pwd)/assets/viewer.html"
[ -f "$template" ] || { echo "error: template not found: $template" >&2; exit 2; }

html_escape() { sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/"/\&quot;/g'; }

sections_tmp="$(mktemp)"
trap 'rm -f "$sections_tmp"' EXIT
for f in "${files[@]}"; do
  label="$(basename "$f")"
  label="${label%.*}"
  label="$(printf '%s' "$label" | awk '{ print toupper(substr($0,1,1)) substr($0,2) }' | html_escape)"
  {
    printf '<section title="%s"><pre class="mermaid">\n' "$label"
    html_escape < "$f"
    printf '\n</pre></section>\n'
  } >> "$sections_tmp"
done

title_esc="$(printf '%s' "$title" | html_escape)"
awk -v secfile="$sections_tmp" -v title="$title_esc" '
  /BLUEPRINT:DIAGRAMS:START/ { skip = 1; while ((getline line < secfile) > 0) print line; next }
  /BLUEPRINT:DIAGRAMS:END/   { skip = 0; next }
  skip { next }
  /<h1 id="title">/ { printf "  <h1 id=\"title\">%s</h1>\n", title; next }
  { print }
' "$template" > "$out"
abs_out="$(cd "$(dirname "$out")" && pwd)/$(basename "$out")"
echo "built: $abs_out (${#files[@]} diagram(s))"

find_browser() {
  local c
  for c in \
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
    "/Applications/Chromium.app/Contents/MacOS/Chromium" \
    "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge" \
    google-chrome chromium chromium-browser microsoft-edge; do
    if command -v "$c" >/dev/null 2>&1; then printf '%s' "$c"; return 0; fi
  done
  return 1
}

if browser="$(find_browser)"; then
  dom="$("$browser" --headless=new --disable-gpu --virtual-time-budget=15000 \
        --dump-dom "file://$abs_out" 2>/dev/null || true)"
  n_errors="$(printf '%s\n' "$dom" | grep -c 'class="blueprint-error"' || true)"
  n_rendered="$(printf '%s\n' "$dom" | grep -c 'aria-roledescription=' || true)"
  if [ "$n_errors" -gt 0 ]; then
    echo "FAIL: $n_errors diagram(s) did not parse:" >&2
    printf '%s\n' "$dom" | awk '/class="blueprint-error"/ { f = 1 } f { print "  " $0 } f && /<\/pre>/ { f = 0 }' >&2
    exit 1
  elif [ "$n_rendered" -lt ${#files[@]} ]; then
    echo "warning: only $n_rendered/${#files[@]} diagrams rendered — offline or CDN blocked? Diagrams NOT validated." >&2
  else
    echo "OK: all ${#files[@]} diagram(s) parsed and rendered"
  fi
else
  echo "warning: no Chrome/Chromium/Edge found — skipped validation" >&2
fi

if [ "$open_after" -eq 1 ]; then
  if command -v open >/dev/null 2>&1; then open "$abs_out"
  elif command -v xdg-open >/dev/null 2>&1; then xdg-open "$abs_out"
  else echo "warning: no opener found; open $abs_out yourself" >&2
  fi
fi
