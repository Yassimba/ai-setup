---
name: update-docstrings
description: Add or update docstrings, doc comments, and inline comments in source code — any language. Use when the user wants docstrings written or fixed, a conversion to a doc style (NumPy, TSDoc, rustdoc), or a comment cleanup. Source files only — README/ADR/prose docs belong to write-documentation / write-readme.
---

# Update Docstrings

"Docstring" below means the language's doc unit, whatever its syntax: a Python `"""` string, a TSDoc `/** */` block, a Rust `///` comment.

**Docs-only diff.** The diff this skill produces changes docstrings and comments, never code — not a signature, not an import, not a blank line inside a body. When the real fix is code — a rename, an extracted helper, a lambda promoted to a named function so it can carry a doc — propose the change and let the user decide; don't make it while "just adding docstrings".

## Foundational principle: self-contained

Every docstring is read by a first-time reader: someone with the source in front of them and nothing else — no PR thread, no tracker, no design docs, no memory of the project's history. A docstring is **self-contained** when it gives that reader everything, in place. Four rules follow:

- **Presence never lapses.** Everything gets a docstring — public and private, trivial and complex; only length scales with the symbol's surface. A construct that syntactically cannot carry one (a `lambda`, an inline callback) needs promoting to a named symbol if it needs explaining — a code change, so propose it.
- **Forward-facing: behaviour, not process.** Write for the reader who arrives after the dust has settled; say what the code does today. "Phase 0", "stub until issue 3", "for now", "lands in issue 2", sprint labels, PRD/ticket links — process narrative rots and means nothing to a first-time reader; it belongs in the PR description or the tracker.
- **State the rule, not the citation.** No "ADR-0019", "per the RFC", "as the design doc says", plan/PRD titles, or issue IDs. The decision may well _live_ in an ADR — that is correct, and it is exactly why the docstring must not point at it: the reader cannot open ADR-0019 from the source. Write the rule the code follows, in plain terms. Bad: `the stable ADR-0019 token path`. Good: `the stable token path; sibling ids are independent, so an unrelated add or delete never renumbers a node`. The only names a docstring may carry are code symbols the reader can resolve in the same codebase (a class, a function, a config key).
- **Gloss project jargon at first use.** The first time a domain term appears in a file, give it a one-line gloss — `a "ksub" (kabelsubgroep) is a bundle of cable segments owned by one transformer` — then use it freely; the reader can grep back to the definition.

## Public vs internal

| Symbol kind                                                                                   | Treatment                                                                                                                          |
| --------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| **Public** — part of the module's API surface (the language reference names the exact signal) | Full docstring in the language's dialect: summary + extended description as needed + parameter / return / error / example sections |
| **Internal** — private by the language's convention                                           | One sentence, plus at most one more on _how_ it works when non-obvious. No parameter / return sections.                            |

The split is about _length_, never _presence_. Full sections on a tiny private helper just restate the signature the reader can already see; one line of intent is what they actually need. Reaching for a parameter section on an internal helper means either the symbol isn't actually internal, or the docstring needs trimming back down.

### Section rules — every dialect

- **Summary**: one line, concrete ("Split a dotted Capability ID into three segments", not "Implements parsing logic for identifiers"), ending with a period. Mood (imperative vs third person) follows the dialect. A summary that only re-says the symbol's name fails — "Gets the name" on `get_name` documents nothing; say what the signature can't.
- **Extended description**: only when the summary needs context — invariants, side effects, or why the symbol exists. Skip for obvious functions.
- **Parameters**: describe _meaning_, not the type the signature already shows.
- **Returns**: what the value _is_. Omit for procedures that return nothing.
- **Errors / Raises / Throws**: failures raised _deliberately_ — not every error reachable through the call graph, and not errors that fire only when the caller breaks a contract the docstring already states.
- **Name constants, don't copy values**: `within STALE_THRESHOLD_SECS`, not "within 5 minutes" — the copied value silently rots when the constant changes. Glossing a literal's unit in place (`1048576` is 1 MiB) is fine; that adds clarity, not a second copy.
- **Examples**: when usage is non-obvious, with real values from the codebase, not placeholders. Prefer the dialect's executable form — an executable example doubles as a smoke test.

## Language dialects

Read the file's language reference before writing — it carries the public/internal signal, section syntax, special cases, and the doc linter:

- **Python** — NumPy style: [references/python.md](references/python.md)
- **TypeScript / JavaScript** — TSDoc: [references/typescript.md](references/typescript.md)
- **Rust** — rustdoc: [references/rust.md](references/rust.md)

No reference for the language? Apply everything above through its dominant convention (javadoc, godoc, XML doc comments, …): same presence rule, same public/internal split, sections in the shape that convention renders.

## Comments

A comment is prose too, and self-contained applies unchanged: behaviour not process, the rule not the citation.

What differs from a docstring is the bias. A docstring's presence never lapses; a comment's must be earned. The code already says _what_ it does — a comment earns its place only by saying _why_: a non-obvious decision, a workaround for an external bug, an invariant the code leans on, an ordering that looks arbitrary but isn't.

The test is altitude. A comment at the same level of abstraction as the code restates it — delete. A comment earns its keep by moving **lower** — precision the code can't show: units, boundary inclusivity, who owns a resource, what null means — or **higher** — the intuition: the why, or the simpler mental model behind the mechanism.

Delete on sight:

- **Commented-out code** — version control remembers it; a dead block in source is noise.
- **Restatement** — `# increment i` above `i += 1`, `// loop over the rows` above the loop. If the comment only re-says the line below it, cut it.
- **Decorative banners and dividers** — `# ---- helpers ----`, `// === main ===`.
- **Doubts and notes-to-self** — `# TODO: this is probably wrong`, `// not sure why this works`. An actionable TODO belongs in the issue tracker with an owner; a vague doubt belongs nowhere.

Keep, but rewrite to forward-facing prose:

- A comment that explains _why_ non-obvious code is the way it is. Strip any process narrative or document citation, apply the writing skill, and leave the bare reason.

If a comment exists only to decode a cryptic name or untangle a confusing line, the fix is the code, not the comment — a rename or an extract. That's a code change: propose it per the docs-only rule rather than refactoring silently.

## Workflow

1. Invoke the `writing-clearly-and-concisely` skill (via the Skill tool) — every docstring and comment is prose under its rules; Strunk's composition rules and the AI-pattern catalogue live there, not duplicated here.
2. Detect the language; read its reference above (or pick the dominant convention).
3. Read the file end-to-end — the bodies, not just the existing docs; mark which symbols are public vs internal.
4. Write in source order — public symbols first in long files, then internal.
5. Re-read each docstring against the body; where they disagree, **the code wins** — rewrite the doc to what the code does today. Then check each is self-contained. When rewriting, preserve modality — _must_, _should_, and _may_ state different obligations; don't swap one for another unless the code says so.
6. Pass over the comments per the Comments section: apply the delete-on-sight list; rewrite survivors to forward-facing prose.
7. Run the doc linter named in the language reference and fix what it flags. Done when every symbol carries a docstring, no remaining comment matches the delete-on-sight list, every survivor states a _why_, and the linter is clean.
