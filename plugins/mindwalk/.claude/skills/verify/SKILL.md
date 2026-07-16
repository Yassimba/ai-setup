---
name: verify
description: Build, launch, and drive the mindwalk web UI end-to-end for verification.
---

# Verifying mindwalk

## Build & launch

```sh
npm --prefix web run build                  # tsc -b && vite build → web/dist
go build -o bin/mindwalk ./cmd/mindwalk
bin/mindwalk serve --no-open --dev --port <PORT> # --dev serves web/dist from the working tree
```

Gotchas:

- Always pass `--no-open` during automated verification so repeated server
  starts do not create tabs or steal focus from the user's current work.
- Port 8765 is the vite-proxy convention and is often already taken by a dev
  server; pick another port and check the log for `bind: address already in use`.
- Sessions come from `~/.claude/projects` — this machine has real data, no
  fixtures needed. `testdata/claude-session.jsonl` works via `mindwalk open`.

## Drive (headless Chrome + CDP, no npm installs)

System Chrome + raw CDP over Node's built-in WebSocket works; WebGL renders
under `--headless=new` with `--use-angle=swiftshader --enable-unsafe-swiftshader`:

```sh
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
  --headless=new --remote-debugging-port=9333 --user-data-dir=<tmp> \
  --window-size=1440,900 --no-first-run about:blank &
```

New-tab endpoint needs PUT: `fetch('http://127.0.0.1:9333/json/new?<url>', {method:'PUT'})`.

## Flows worth driving

- Load via `/?session=<id>` deep link; assert `.session-row.active .session-title`.
- Readout `.readout-count` shows `N / N` after load (playhead starts at end).
- `[aria-label="Restart playback"]` → `1 / N`; `[aria-label="Play playback"]`
  → ~3 ticks/second; playback draws the ember trail + firefly.
- View toggle `Tree` / `Terrain` buttons rebuild the scene — watch for
  `Runtime.exceptionThrown`.
- Rapid session switch (click uncached row, 150ms later a cached row) must end
  showing the last-clicked session's data.
- Bogus `?session=` must fall back to the newest session with a console.warn.
