---
name: write-documentation
description: 'Write software documentation structured by Diátaxis. Use when the user wants a tutorial, how-to guide, reference page, explanation doc, or user guide — "write docs", "document this", "we need a guide for X". Not for READMEs (write-readme) or docstrings (update-docstrings).'
---

# Write Documentation (Diátaxis)

Produce one document per run, typed by the Diátaxis quadrant it serves: a **tutorial** (a lesson), a **how-to guide** (a recipe), **reference** (a dictionary), or an **explanation** (a discussion).

## Workflow

1. **Classify.** Place the request on the Diátaxis compass:
   - Does the document serve the reader's _action_ (they are doing something) or _cognition_ (they are understanding something)?
   - Does it serve _acquisition_ (they are studying) or _application_ (they are working)?

   |               | acquisition (study) | application (work) |
   | ------------- | ------------------- | ------------------ |
   | **action**    | tutorial            | how-to guide       |
   | **cognition** | explanation         | reference          |

   A request that spans quadrants is more than one document — say so and pick the primary one for this run.

2. **Confirm the brief.** State your classification and ask about whatever is still unknown of:
   - **Audience** — who reads this, and what do they already know?
   - **Reader's goal** — what can they do or understand after reading?
   - **Scope** — what is in, and what is explicitly out?

   Done when document type, audience, goal, and scope are each either answered by the user or proposed by you and accepted.

3. **Load the quadrant's rules.** Read the matching file before outlining:
   - Tutorial → [references/tutorial.md](references/tutorial.md)
   - How-to guide → [references/how-to.md](references/how-to.md)
   - Reference → [references/reference.md](references/reference.md)
   - Explanation → [references/explanation.md](references/explanation.md)

4. **Propose an outline.** A table of contents, one line per section, shaped by the quadrant's rules. Await approval before writing.

5. **Write.** Done when every section of the approved outline is fully written — none stubbed, summarized, or deferred.

## Sources

Work only from material the user provides and the codebase at hand. Provided markdown files calibrate tone and terminology; quote them only on request. Verify every code snippet against the actual code before including it.
