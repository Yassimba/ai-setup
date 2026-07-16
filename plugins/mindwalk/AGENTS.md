# AGENTS.md

`mindwalk` is a local visualizer for coding-agent sessions. It supports Claude Code and Codex, turning agent session logs plus repository structure into a deterministic 3D "code city" that can be explored in a browser.

## Design

The project has two primary artifacts:

- A normalized trace of what happened during a supported coding-agent session.
- A deterministic citymap of the repository being edited or inspected.

The UI combines those artifacts so users can see how a coding agent moved through a codebase over time. Keep this separation clear: source-specific parsing should not know about rendering, citymap generation should not depend on session playback, and the server should mainly connect data sources to the web client.

## Architecture

- `cmd/mindwalk` provides the CLI commands: serve a local UI, open a session, build a citymap, or export a trace.
- `internal/adapter` converts supported agent session formats into the shared model. Claude Code and Codex each have an adapter; keep every source, current and future, behind its adapter boundary.
- `internal/model` owns the trace and citymap data contracts.
- `internal/citymap` builds deterministic layouts from repository contents.
- `internal/server` exposes local APIs and serves the web app. `internal/server/static` holds the embedded frontend assets generated from `web/dist`.
- `web` contains the React, Vite, and Three.js frontend.
- `schema` mirrors the exported JSON contracts.

The normal flow is:

```text
Agent session log (Claude Code or Codex) + repository path
  -> Go adapters and citymap builder
  -> local Go server APIs
  -> React/Three.js playback UI
```

## Development

- Use `make setup` to install frontend dependencies.
- Use `make test` for the standard validation pass.
- Use `make serve` for local development.
- Use `make build` when refreshing the distributable binary and embedded frontend assets.

Keep Go code formatted with `gofmt`. Do not hand-edit `internal/server/static`; when bundled assets need to change, regenerate them with `make build` (or `make embed-static`). When trace or citymap JSON shapes change, update `schema` and the relevant tests in the same change.
