# Web UI Walkthrough Recipes (agent-browser)

Drive the browser first-hand with `agent-browser`; capture screenshots and console output as evidence.

## Platform & install

`agent-browser` requires Linux, WSL, or macOS:

```bash
uname -s   # Linux or Darwin → proceed; otherwise skip web UI

agent-browser --version   # check
# if not found:
npm install -g agent-browser
agent-browser install --with-deps
```

If install fails, warn and continue with CLI + API only.

## Core commands

```bash
agent-browser open <url>
agent-browser snapshot -i              # get interactive refs @e1, @e2, ...
agent-browser click @eN
agent-browser fill @eN "text"
agent-browser select @eN "option"
agent-browser press Enter
agent-browser screenshot <path>
agent-browser screenshot --annotate    # marks interactive elements
agent-browser set viewport W H         # e.g. 375 812
agent-browser wait --load networkidle
agent-browser console                  # JS errors
agent-browser errors                   # uncaught exceptions
agent-browser get text @eN
agent-browser get url
agent-browser close
```

**Refs invalidate after navigation or DOM changes.** Always re-snapshot before the next interaction.

## Per-step loop

1. Snapshot → refs
2. Perform interaction (click / fill / select / press)
3. Wait for settle (`wait --load networkidle`)
4. Screenshot → `ai-docs/e2e-screenshots/<journey>/<NN>-<desc>.png`
5. **Read the screenshot back via `Read`** — check for visual correctness, broken layouts, missing content, error states.
6. Periodically run `agent-browser console` and `agent-browser errors`

## Responsive sweep

Have a dedicated journey for these. Revisit each key page at:

```bash
agent-browser set viewport 375 812    # Mobile
agent-browser set viewport 768 1024   # Tablet
agent-browser set viewport 1440 900   # Desktop
```

Screenshot at each viewport. Check overflow, broken alignment, touch-target sizes.
