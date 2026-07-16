---
name: mermaid-skill
description: Generate Mermaid diagrams as code (.mmd) and export to PNG/SVG/PDF with mmdc. Use when the user wants a diagram — flowchart, sequence, class, ER, state, gantt, git graph, architecture, timeline, mindmap — proactively when explaining a system with 3+ components, a runtime flow, a schema, or a lifecycle, and when another skill (e.g. blueprint) needs a diagram authored or exported to an image.
---

# Mermaid Diagrams

Author `.mmd` text files and compile them to images with `mmdc`. Mermaid lays everything out automatically — no coordinates — and the text source lives in git and embeds in Markdown.

Route elsewhere when the request needs: pixel-precise placement, branded icons, or heavy styling → **drawio**; a hand-drawn aesthetic → **excalidraw**/**tldraw**; a freeform whiteboard → **tldraw**; strict conventional UML notation → **plantuml**.

## Setup

`mmdc` renders through Puppeteer and needs a headless Chrome:

```bash
npm install -g @mermaid-js/mermaid-cli
npx puppeteer browsers install chrome-headless-shell   # mmdc has no bundled browser
```

**`Could not find Chrome` is a toolchain failure, never a diagram error.** `mmdc --version` passes without Chrome, and a perfectly valid `.mmd` still fails to compile. Install the browser above or set `PUPPETEER_EXECUTABLE_PATH` to a system Chrome; leave the `.mmd` alone.

## Workflow

1. **Author** — pick the type from the shape of the content:

   | The content is…                                     | Type                                     |
   | ---------------------------------------------------- | ---------------------------------------- |
   | steps, branches, dependencies                         | `flowchart`                              |
   | who calls whom, in what order                         | `sequenceDiagram`                        |
   | tables and relations / types and inheritance          | `erDiagram` / `classDiagram`             |
   | statuses one thing moves through                      | `stateDiagram-v2`                        |
   | deployed services and their wiring                    | `architecture-beta`                      |
   | schedule / branch history / proportions / idea tree   | `gantt` / `gitGraph` / `pie` / `mindmap` |
   | events over time / effort-impact grid / plotted data  | `timeline` / `quadrantChart` / `xychart-beta` |

   One diagram answers one question — past ~20 nodes, split by concern into separate `.mmd`s instead of cramming.
   Syntax references, consult when unsure:
   [FLOWCHART](reference/FLOWCHART.md) · [SEQUENCE](reference/SEQUENCE.md) · [CLASS-ER](reference/CLASS-ER.md) · [ARCHITECTURE](reference/ARCHITECTURE.md) (`architecture-beta` — newer syntax, read before first use) · [OTHER-TYPES](reference/OTHER-TYPES.md) (state, gantt, gitGraph, pie, mindmap, journey, timeline, quadrant, xychart, C4).
   Done when every node, participant, and label names a real thing from the content being diagrammed — no placeholders.

2. **Compile** — the compile is the syntax check; a parse error is a bug in the `.mmd`, fix and re-compile until it renders:

   ```bash
   mmdc -i diagram.mmd -o diagram.png -w 2048
   ```

   Themes: `-t default|dark|neutral|forest` (`base` works only inside a `%%{init: {'theme':'base'}}%%` directive). SVG/PDF: same command with `.svg`/`.pdf` output.

3. **Self-check (vision)** — a clean compile only proves the syntax; read the exported PNG and judge it as its audience will. Mermaid positions everything itself, so look for content defects, not overlaps:

   | Check              | What to look for                      | Fix                                                                         |
   | ------------------ | ------------------------------------- | --------------------------------------------------------------------------- |
   | Label truncation   | Node / edge text clipped              | Shorten the label, or wrap with `<br/>`                                     |
   | Cramped density    | Nodes crammed together; tangled lines | Flip direction (`TD`↔`LR`), split into `subgraph`s, or reduce nodes         |
   | Wrong aspect       | Far too wide or too tall to read      | Change `flowchart TD`↔`LR` (or set `direction` in class/state)              |
   | Edge spaghetti     | Many crossings, hard to follow        | Reorder declarations so connected nodes sit adjacent; group with `subgraph` |
   | Wrong diagram type | Type doesn't suit the content         | Re-pick from the step-1 table                                               |
   | Low contrast       | Text blends into the node fill        | `classDef` ([FLOWCHART § Styling](reference/FLOWCHART.md)) or `-t` theme    |

   Re-compile after every fix. Max **2 self-check rounds**, then show the user regardless. No vision available → skip straight to the review loop.

4. **Review loop** — show the image, apply the minimal `.mmd` edit per request, re-compile. Overwrite the same `.mmd`/`.png` each round — no `v1`, `v2`, … After **5 rounds**, suggest the user fine-tune at [mermaid.live](https://mermaid.live).

5. **Report** — give the user the `.mmd` and image paths.

## Gotchas

| Symptom                                              | Fix                                                                     |
| ---------------------------------------------------- | ----------------------------------------------------------------------- |
| Parse error on a label with special chars            | Quote it: `A["Label: value"]`                                           |
| Parse error on a subgraph name with spaces           | Quote it: `subgraph "My Layer"`                                         |
| `Maximum text size in diagram exceeded`              | Split the diagram; last resort `-c config.json` `{"maxTextSize": 200000}` |
| Chrome crashes in CI / as root                       | `-p puppeteer.json` with `{"args": ["--no-sandbox"]}`                   |
| Architecture icons missing / `logos:` name not found | Register the pack at compile time — [ARCHITECTURE § Icons](reference/ARCHITECTURE.md) |
| Blank or tiny PNG                                    | Add `-w 2048`                                                           |
