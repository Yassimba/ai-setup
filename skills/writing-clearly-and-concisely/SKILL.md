---
name: writing-clearly-and-concisely
description: Use when writing prose humans will read—documentation, commit messages, error messages, reports, UI text—or when the user wants existing text to sound human rather than AI-written ("humanize this", "reads like ChatGPT").
---

# Writing Clearly and Concisely

## Overview

Write vigorous prose. Vigorous writing is concise: every word tells, sentences are active and concrete, needless words are gone. This skill pairs Strunk's _The Elements of Style_ with a field guide to AI writing patterns.

## Principles

Strunk's Elementary Principles of Composition — apply these to everything you write:

- One paragraph per topic; begin it with a topic sentence
- **Use active voice**
- **Put statements in positive form**
- **Use definite, specific, concrete language**
- **Omit needless words**
- Express co-ordinate ideas in similar form; keep related words together
- **Place emphatic words at the end of the sentence**

## What to load

Pick the row that matches the task and load exactly one file — the full pass below is the only exception:

| Task                                                        | Load                                                                         |
| ----------------------------------------------------------- | ---------------------------------------------------------------------------- |
| Writing or editing paragraphs, docs, explanations           | `references/elements-of-style/03-elementary-principles-of-composition.md`    |
| Fixing grammar, commas, punctuation in existing text        | `references/elements-of-style/02-elementary-rules-of-usage.md`               |
| Choosing the right word, fixing common misuses              | `references/elements-of-style/05-words-and-expressions-commonly-misused.md`  |
| Headings, quotations, formatting                            | `references/elements-of-style/04-a-few-matters-of-form.md`                   |
| Making text sound human ("humanize", "reads like ChatGPT")  | `references/humanizer.md`                                                     |
| Auditing whether text is AI-written (deep pattern catalogue) | `references/signs-of-ai-writing.md`                                          |

### Full pass

Load the bundle — `02`, `03`, `04`, `05`, `references/humanizer.md`, and `references/ai-patterns.md` — when, and only when, the user explicitly signals a comprehensive job:

- "write the documentation" / "write docs for X" / "draft the README" (a substantial new piece of prose, not a one-paragraph edit)
- "do a full pass" / "thorough edit" / "deep edit" / "go through all the rules"
- "load everything" / "use the full skill" / "no shortcuts"

"Fix this paragraph" or "tighten this sentence" stays with the single-file default.

## AI Writing Patterns to Avoid

LLMs regress to statistical means, producing generic, puffy prose. Say what the thing actually does, in the words a knowledgeable human would pick. Guardrails:

- **Puffery:** pivotal, crucial, vital, testament, enduring legacy
- **Empty "-ing" phrases:** ensuring reliability, showcasing features, highlighting capabilities
- **Promotional adjectives:** groundbreaking, seamless, robust, cutting-edge
- **Overused AI vocabulary:** delve, leverage, multifaceted, foster, realm, tapestry
- **Formatting overuse:** excessive bullets, emoji decorations, bold on every other word

When these bullets aren't enough, load `references/ai-patterns.md` — the distilled field guide with words-to-watch per pattern. Reserve `references/signs-of-ai-writing.md` (the full Wikipedia catalogue with examples) for explicit "is this AI-written?" audits.

## Done means

A writing or editing pass is finished when a re-read of the full output finds:

- no needless word left — each word either informs or goes
- no passive where the active would work
- nothing generic where a specific would do
- none of the AI patterns above

If the re-read finds a violation, fix it and re-read again.
