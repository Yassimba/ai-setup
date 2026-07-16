# Visual Companion Guide

Browser-based companion for showing mockups, diagrams, and options during brainstorming.

## When to use the browser

Decide per-question, not per-session. The test: **would the user understand this better by seeing it than reading it?** Show in the browser what is itself visual — UI mockups and layouts, architecture and flow diagrams, side-by-side design comparisons, look-and-feel questions, spatial structures (state machines, entity maps). Keep in the terminal what resolves in words — scope, trade-off lists, conceptual A/B choices, clarifying questions.

A question *about* a UI topic is not automatically visual: "What kind of wizard do you want?" is conceptual — terminal. "Which of these wizard layouts feels right?" is visual — browser.

## How it works

The server watches `screen_dir` and serves the newest HTML file to the browser. The user clicks to select options; selections are recorded to `state_dir/events`, which you read on your next turn.

## Starting a session

```bash
# Start server with persistence (mockups saved to project)
scripts/start-server.sh --project-dir /path/to/project

# Returns: {"type":"server-started","port":52341,"url":"http://localhost:52341",
#           "screen_dir":"/path/to/project/ai-docs/brainstorm/12345-1706000000/content",
#           "state_dir":"/path/to/project/ai-docs/brainstorm/12345-1706000000/state"}
```

Save `screen_dir` and `state_dir` from the response. Tell the user to open the URL.

**Finding connection info:** the server writes its startup JSON to `$STATE_DIR/server-info`. If you launched it in the background and didn't capture stdout, read that file for the URL and port. When using `--project-dir`, the session directory is under `<project>/ai-docs/brainstorm/`.

**Persistence:** pass the project root as `--project-dir` so mockups survive server restarts in `ai-docs/brainstorm/`; without it, files go to `/tmp` and get cleaned up. If `ai-docs/` isn't gitignored yet, remind the user to add it (`setup-project` normally does this); mockups worth keeping get promoted into the plan directory by the skill's save flow.

**Keeping the server alive:** it must keep running across conversation turns.

- **Claude Code (macOS/Linux)** — the script backgrounds itself; run it normally.
- **Claude Code (Windows)** — auto-detects and runs foreground; set `run_in_background: true` on the Bash call, then read `$STATE_DIR/server-info` next turn.
- **Codex** — auto-detects `CODEX_CI` and runs foreground; run it normally.
- **Gemini CLI** — pass `--foreground` and set `is_background: true` on the shell call.
- **Other environments that reap detached processes** — pass `--foreground` and launch with your platform's background mechanism.

If the URL is unreachable from the browser (common in remote/containerized setups), bind a non-loopback host; `--url-host` controls the hostname printed in the returned URL:

```bash
scripts/start-server.sh \
  --project-dir /path/to/project \
  --host 0.0.0.0 \
  --url-host localhost
```

## The loop

1. **Check the server is alive, then write a screen.** If `$STATE_DIR/server-info` is missing or `$STATE_DIR/server-stopped` exists, the server has shut down (it auto-exits after 30 minutes of inactivity) — restart it before continuing. Write the HTML with the Write tool (cat/heredoc dumps noise into the terminal) to a *new* semantically named file in `screen_dir` — `platform.html`, `layout.html`, iterations as `layout-v2.html`. The server serves the newest file by modification time, so never reuse a filename.
2. **Tell the user what to expect and end your turn.** Remind them of the URL every step, summarize what's on screen ("Showing 3 layout options for the homepage"), and ask them to respond in the terminal: "Take a look and let me know what you think. Click to select an option if you'd like."
3. **On your next turn**, read `$STATE_DIR/events` if it exists and merge it with the user's terminal text — the terminal message is the primary feedback; events add structured interaction data.
4. **Iterate or advance.** If feedback changes the current screen, write a new version; move to the next question only when the current one is validated.
5. **Unload when returning to terminal.** When the next step is textual (a clarifying question, a trade-off discussion), push a waiting screen so the user isn't staring at a resolved choice while the conversation moves on:

   ```html
   <!-- filename: waiting.html (or waiting-2.html, etc.) -->
   <div style="display:flex;align-items:center;justify-content:center;min-height:60vh">
     <p class="subtitle">Continuing in terminal...</p>
   </div>
   ```

6. Repeat until done.

## Writing content fragments

Write just the content that goes inside the page. Unless your file starts with `<!DOCTYPE` or `<html`, the server wraps it in the frame template — header, CSS theme, selection indicator, and all interactive infrastructure. Write a full document only when you need complete control over the page.

**Minimal example:**

```html
<h2>Which layout works better?</h2>
<p class="subtitle">Consider readability and visual hierarchy</p>

<div class="options">
  <div class="option" data-choice="a" onclick="toggleSelect(this)">
    <div class="letter">A</div>
    <div class="content">
      <h3>Single Column</h3>
      <p>Clean, focused reading experience</p>
    </div>
  </div>
  <div class="option" data-choice="b" onclick="toggleSelect(this)">
    <div class="letter">B</div>
    <div class="content">
      <h3>Two Column</h3>
      <p>Sidebar navigation with main content</p>
    </div>
  </div>
</div>
```

That's it. No `<html>`, no CSS, no `<script>` tags needed.

## CSS classes available

The frame template provides these classes for your content:

### Options (A/B/C choices)

```html
<div class="options">
  <div class="option" data-choice="a" onclick="toggleSelect(this)">
    <div class="letter">A</div>
    <div class="content">
      <h3>Title</h3>
      <p>Description</p>
    </div>
  </div>
</div>
```

**Multi-select:** add `data-multiselect` to the container to let users select multiple options. Each click toggles the item; the indicator bar shows the count.

```html
<div class="options" data-multiselect>
  <!-- same option markup — users can select/deselect multiple -->
</div>
```

### Cards (visual designs)

```html
<div class="cards">
  <div class="card" data-choice="design1" onclick="toggleSelect(this)">
    <div class="card-image"><!-- mockup content --></div>
    <div class="card-body">
      <h3>Name</h3>
      <p>Description</p>
    </div>
  </div>
</div>
```

### Mockup container

```html
<div class="mockup">
  <div class="mockup-header">Preview: Dashboard Layout</div>
  <div class="mockup-body"><!-- your mockup HTML --></div>
</div>
```

### Split view (side-by-side)

```html
<div class="split">
  <div class="mockup"><!-- left --></div>
  <div class="mockup"><!-- right --></div>
</div>
```

### Pros/Cons

```html
<div class="pros-cons">
  <div class="pros"><h4>Pros</h4><ul><li>Benefit</li></ul></div>
  <div class="cons"><h4>Cons</h4><ul><li>Drawback</li></ul></div>
</div>
```

### Mock elements (wireframe building blocks)

```html
<div class="mock-nav">Logo | Home | About | Contact</div>
<div style="display: flex;">
  <div class="mock-sidebar">Navigation</div>
  <div class="mock-content">Main content area</div>
</div>
<button class="mock-button">Action Button</button>
<input class="mock-input" placeholder="Input field">
<div class="placeholder">Placeholder area</div>
```

### Typography and sections

- `h2` — page title
- `h3` — section heading
- `.subtitle` — secondary text below title
- `.section` — content block with bottom margin
- `.label` — small uppercase label text

## Browser events format

User clicks are recorded to `$STATE_DIR/events` (one JSON object per line). The file is cleared automatically when you push a new screen.

```jsonl
{"type":"click","choice":"a","text":"Option A - Simple Layout","timestamp":1706000101}
{"type":"click","choice":"c","text":"Option C - Complex Grid","timestamp":1706000108}
{"type":"click","choice":"b","text":"Option B - Hybrid","timestamp":1706000115}
```

The full stream shows the user's exploration path — they may click several options before settling. The last `choice` event is typically the final selection, but the click pattern can reveal hesitation worth asking about. If the file doesn't exist, the user didn't interact with the browser — use only their terminal text.

## Design tips

- **Scale fidelity to the question** — wireframes for layout, polish for polish questions
- **Explain the question on each page** — "Which layout feels more professional?" not just "Pick one"
- **2–4 options max** per screen
- **Use real content when it matters** — for a photography portfolio, actual images (Unsplash); placeholder content obscures design issues
- **Keep mockups simple** — layout and structure over pixel-perfect design

## Cleaning up

```bash
scripts/stop-server.sh $SESSION_DIR
```

Sessions started with `--project-dir` keep their mockups in `ai-docs/brainstorm/` for later reference; only `/tmp` sessions are deleted on stop.

## Reference

- Frame template (CSS reference): `scripts/frame-template.html`
- Helper script (client-side): `scripts/helper.js`
