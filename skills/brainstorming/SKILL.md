---
name: brainstorming
description: Use when the user wants to brainstorm or explore an idea — a feature, product direction, or "what if" — before deciding whether it deserves a plan. Ideation only — ends in an idea brief, not a design.
---

# Brainstorming

Turn a fuzzy idea into a clear direction through conversation. Brainstorming answers **what are we trying to do?** — *how will we build it?* belongs to `grill-with-docs`, so when the conversation drifts toward architecture, code, or "how should this fit this repo?", stop and offer to move there. The brief below is the only artifact this skill produces: no specs, issues, commits, or code.

If invoked with no idea attached, read `references/gamechanging-feature.md` and follow it to surface a candidate.

## Diverge

1. **Name the raw idea.** Restate it in one or two sentences so the user can correct you early.
2. **Ask one question per turn** about purpose, audience, pain, outcome, constraints, and non-goals. Make each easy to answer: concrete options, multiple choice when it helps, your recommended answer included.
3. **Sketch what has shape.** When the idea is visual — UI, workflow, comparison, diagram, mental model — a sketch beats another paragraph, so reach for the companion routinely. Offer it in its own message and wait:
   > Want me to spin up a local browser companion for sketches and diagrams? It's token-heavier but often clearer than text. (Requires opening a local URL.)
   If they agree, read `references/visual-companion.md` and follow it; if they decline, continue text-only.
4. **Offer 2–3 directions** with plain-language trade-offs.

## Converge

When you can fill every field of the brief from the user's own answers — not your guesses — produce it:

```md
## Idea brief

Idea:

Audience:

Problem / pain:

Desired outcome:

Non-goals:

Promising directions:
1.
2.
3.

Recommended direction:

Open questions for `grill-with-docs`:
-
-
```

Then ask (use AskUserQuestion if available):

> Want to run `grill-with-docs` to refine this idea further?
> Want to save this idea to the backlog to pick up later?

If they save it: write the brief to `ai-docs/plans/<YYYY-MM-DD-project>/idea.md` (today's date, short kebab-case project slug; create the directory), and if the visual companion ran, copy its mockup HTML files from the session's `screen_dir` into `mockups/` beside the brief. Then add it to the backlog using the backlog skill. In that same directory, `/to-spec` will later write `spec.md` and `/to-tickets` will write `issues/<NNN>-<name>.md`.
