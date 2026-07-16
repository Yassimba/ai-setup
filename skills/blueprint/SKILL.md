---
name: blueprint
description: Blueprint a change before building it — Mermaid diagrams of the design, held for approval before any code. Use before implementing non-trivial work — proactively or when the user asks to see the design first — or when another skill needs a design-approval gate.
---

# Blueprint

Show the human a picture of what will be built, get sign-off, then build. The blueprint is the contract for the work that follows.

## 1. Draft

Understand the change from whatever input exists - a user description, spec, ticket, or plan file - then read the code it touches.

Decide the axes by trigger - draw every axis whose trigger fires in this change:

| Axis      | Mermaid type      | Draw when the change has…                                             |
| --------- | ----------------- | --------------------------------------------------------------------- |
| Flow      | `flowchart`       | branches, decisions, or distinct end states                           |
| Sequence  | `sequenceDiagram` | two or more components calling each other at runtime                  |
| Structure | `classDiagram`    | a new or reshaped interface — new class, new/changed public methods   |
| State     | `stateDiagram-v2` | a lifecycle — states an entity moves through, new/changed transitions |
| Data      | `erDiagram`       | a new or reshaped schema — tables, columns, relationships             |

A small change fires less triggers; that one diagram is then the whole blueprint. Skipping a fired axis is a decision — name it and the reason in the chat summary.

Scope each diagram to the delta plus its immediate neighbours — the change, not the codebase. Visually mark **new** versus **existing** (`:::new` + `classDef`, `<<new>>` stereotype, a `(new)` label suffix) so the reader sees at a glance what will be built.

Write each diagram to its own `.mmd` file — beside the plan artifacts if the work has a docs directory, otherwise a temp location. Syntax lives in the **mermaid-skill** — invoke it when unsure of syntax or when a diagram fails to validate, and take only its Author step and reference files: Render below replaces its compile-to-image workflow.

The draft is done when every fired axis has its diagram and every node, participant, and class names an actual file, module, or actor, or one this change creates; a blueprint of generic boxes approves nothing.

## 2. Render

The human approves a picture, not source. The bundled script owns the viewer HTML — build with it every time:

```bash
<skill-dir>/scripts/render.sh -t "<change name>" flow.mmd sequence.mmd
```

It fills the tabbed viewer (filename becomes the tab label), escapes the sources — write plain Mermaid, `<<stereotypes>>` included — and validates every diagram in a headless browser; a parse failure prints the Mermaid error and exits 1. Fix the `.mmd` and re-run until it reports OK.

Show the built page through an inline render/preview tool if the harness has one, otherwise `--open`. If the script warns that validation was skipped or nothing rendered (no browser, or offline — the viewer loads Mermaid from a CDN), emit fenced `mermaid` blocks in chat instead, or export each `.mmd` to an image via the **mermaid-skill**.

Alongside the picture, put a lean summary in chat: what gets built, in what order, and what stays untouched — enough to approve without leaving the conversation.

## 3. Gate

No production code until the blueprint is approved. Ask for a verdict (via AskUserQuestion when available):

- **Approve** — the blueprint is locked; it is the map for the build. Start building.
- **Revise** — take the feedback, redraw, re-render, return to this gate.
- **Rethink** — the approach is wrong; return to Draft from scratch.

If the human asks for a revision you believe is a mistake, explain the trade-off before complying — then draw what they chose.
