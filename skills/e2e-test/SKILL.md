---
name: e2e-test
description: Manual end-to-end walkthrough — drive the live app first-hand (CLI, API, web UI) and query the datastore directly. Trigger on "e2e", "smoke test", "manually test".
---

# E2E Manual Walkthrough

## The evidence rule

Every "it works" claim in this walkthrough rests on **first-hand evidence**: an artifact on disk — `ai-docs/e2e-output/**.txt`, `ai-docs/e2e-screenshots/**.png`, a datastore query result — produced by you driving the **live app**, and read back by you afterward. A passing test suite is **hearsay**: a synthetic harness (TestClient, fixtures, fakes, mocked browsers) vouching for the app from inside. Hearsay is inadmissible — it misses what breaks in production: startup ordering, real network paths, real datastore constraints, real auth, real rendering. A step whose only support is a green test run is an untested step; go drive it.

The deliverables are the evidence trail and the report — plus, when you catch a bug, one regression unit test in the existing suite (see "When you find a real bug").

First-hand evidence per surface:

- **CLI** → `Bash` runs the binary; read stdout, stderr, exit code
- **API** → `curl` / `httpx` against the running server; read status + body
- **Web UI** → `agent-browser` clicks / fills / screenshots; `Read` each screenshot
- **Datastore** (DB / KV / files / queue) → query it directly; confirm the rows / keys / files / messages landed

## Pre-flight — inventory the surfaces

Discover what this project actually exposes:

1. **CLIs** — `[project.scripts]`, `package.json` `bin`, `Cargo.toml` `[[bin]]`, `go.mod` + `cmd/`, user-facing Makefile / justfile targets, scripts in `bin/`
2. **APIs / servers** — framework app instantiations (FastAPI / Flask / Express / Hono / Axum / `net/http` / gRPC …)
3. **Web UIs** — frontend manifests (`vite.config.*`, `next.config.*`, `index.html`, a `package.json` with `dev`/`start`) or static HTML mounted by a backend
4. **Datastores & side-effect systems** — `.env.example` (never `.env`), config files, `docker-compose.yml`; note the client you'll query each with (`psql`, `mysql`, `sqlite3`, `mongosh`, `redis-cli`, `aws s3 ls`, `ls`, `kafkacat`, …)
5. **Tooling** — the package manager / runner this project actually uses (README + manifest)

Write the inventory to `ai-docs/e2e-output/00-inventory.md` — every later phase is scoped to it. If nothing user-facing exists, stop: "This project has no user-facing interfaces — manual E2E needs something to drive." If a web UI exists, check `agent-browser` availability per `references/browser-recipes.md`.

## Phase 1 — Parallel research

Launch THREE sub-agents in parallel via the `Agent` tool in ONE message — app structure & journeys, datastore schema & data flows, bug hunt. Prompts in `references/research-prompts.md`; fill their placeholders from the pre-flight inventory. Wait for all three — their output drives every later phase.

## Phase 2 — Start the app

Install deps and start each service with the project's own tooling (commands came from research), then capture the first evidence:

```bash
<runner> <start-command> &
until curl -sf http://localhost:<port>/<healthz-or-root>; do sleep 0.5; done
<cli-invocation> --help > ai-docs/e2e-output/00-help.txt 2>&1                        # if a CLI exists
agent-browser open http://localhost:<port>
agent-browser screenshot ai-docs/e2e-screenshots/00-initial-load.png                 # if a web UI exists
```

## Phase 3 — Build the task list

`TaskCreate` one task per journey from sub-agent 1, only for surfaces that exist: `[CLI] Test <journey>`, `[API] Test <journey>`, `[Web] Test <journey>`, `[Cross] <journey>` (data flowing between two surfaces).

Then add the **edge-case sweeps** — full journeys in their own right, and where most real bugs live. Each recipe file defines its sweep; the task just names it:

- `[CLI] Error-handling sweep` — `references/cli-recipes.md`
- `[API] Auth & validation sweep` — `references/api-recipes.md`
- `[Web] Responsive sweep` — `references/browser-recipes.md`
- `[Cross] Data consistency across interfaces` — if more than one surface

Then add the **probes** — one `[Probe] <suspected bug>` task per high-priority finding from sub-agent 3's hunt. A finding from static analysis is hearsay until you drive the suspect path in the live app and it reproduces (or fails to).

The walkthrough is complete when every task — journeys, sweeps, AND probes — is completed.

## Phase 4 — Walk every journey, by hand

Per task: `TaskUpdate` → `in_progress`, then drive it yourself. Recipes for the surfaces this project has: `references/cli-recipes.md`, `references/api-recipes.md`, `references/browser-recipes.md`, `references/db-validation.md`.

Every step has the same shape:

1. Drive the interface — run the command / send the request / click the button.
2. Capture evidence — `ai-docs/e2e-output/<journey>/<step>-*.txt`, `ai-docs/e2e-screenshots/<journey>/<NN>-*.png`.
3. **Read the evidence back** — open the output, `Read` the screenshot.
4. **Query the datastore** — whatever sub-agent 2 said this action writes, confirm it landed (`references/db-validation.md`).
5. Cross-interface journeys: act via A, query the datastore, read via B. Both directions.

Tag everything you create with an `e2e-` marker (in names, emails, titles, keys, filenames) — Phase 5 finds and removes test data by that marker.

A journey closes only on admissible evidence: every step has an artifact on disk that you read back, and the datastore confirms every expected side effect. Then `TaskUpdate` → `completed`.

### When you find a real bug

Real = a defect in the app, not a flaky harness or a missing dep.

1. Document expected vs actual with evidence file paths.
2. Write a focused regression unit test in the existing suite — the only test code this skill produces; the `tdd` skill handles the watched-RED.
3. RED → fix via `diagnosing-bugs` → GREEN.
4. Re-drive the failing E2E step first-hand and capture fresh evidence.

## Phase 5 — Cleanup

Kill the processes you started and `agent-browser close`. Then remove the test data: query the datastore for the `e2e-` marker, delete what it finds, and re-query — cleanup is done when the marker query returns nothing. Keep `ai-docs/e2e-output/` and `ai-docs/e2e-screenshots/` — that's the evidence trail.

## Phase 6 — Report

Always emit a text summary in the final message:

```markdown
## Manual E2E Walkthrough Complete

**Interfaces driven:** <e.g. "CLI, API (http://localhost:8000), Web UI (http://localhost:3000)">
**Journeys walked:** <count> (<N> CLI, <N> API, <N> Web, <N> cross — including <N> edge-case sweeps and <N> probes)
**Datastore validation queries run:** <count>
**Evidence files captured:** <count> outputs, <count> screenshots
**Issues found:** <count> (<fixed> fixed, <remaining> remaining)

### Issues fixed during the walkthrough

- [Interface] <description> — `<file:line>` — regression test at `<path>`

### Remaining issues

- [Interface] <description> — <severity> — `<file:line>`

### Probe results

- <sub-agent 3 finding> — confirmed (see issues above) / not reproduced — `<file:line>`

### Artifacts

- `ai-docs/e2e-output/`, `ai-docs/e2e-screenshots/`
```

Then ask the user (via `AskUserQuestion`) whether to write a full `ai-docs/e2e-test-report.md` with per-journey breakdowns, screenshots, datastore checks, and findings. If yes, write it.
