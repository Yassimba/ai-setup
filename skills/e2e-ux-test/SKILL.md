---
name: e2e-ux-test
description: Adversarial UX/DX test — drive the product's live surface (web UI, CLI, code library, or REST API) as its worst-case persona to hunt friction, then filter the rant into RED/YELLOW/WHITE/GREEN verdicts. Trigger on "ux test", "dx test", "asshole user test".
---

# Adversarial UX Test

`e2e-test` asks _does it work?_ — this skill asks _would a human tolerate it?_ You hunt **friction**, not bugs: jargon, click-mazes, cold-start dead ends, error messages that name the symptom but not the fix. The method is the mom test, but angry: fully inhabit the worst-case **persona**, rant in their voice, then break character and filter the rant like a product manager. The filter is what makes this an instrument instead of entertainment.

Three invariants, all surfaces:

- **Evidence rule** — every complaint rests on a captured artifact (your playbook says which kind) under `ai-docs/ux-test/<slug>/` that you `Read` back after capturing.
- **Surface only** — the persona sees what a real user sees: rendered pages, `--help`, published docs and types. Never the implementation source.
- **In character from first contact through the end of the rant. Break character only at the filter.**

## Phase 0 — The surface

Pick the surface under test and read its playbook before anything else:

- **Web UI** → [`references/web.md`](references/web.md)
- **CLI** → [`references/cli.md`](references/cli.md)
- **Code API / library / SDK** → [`references/library.md`](references/library.md)
- **REST / HTTP API** → [`references/rest.md`](references/rest.md)

The playbook defines how to drive the surface, the evidence artifact, the **effort unit**, the friction categories, the **RED ceiling**, and the perception caveat. A product with several surfaces gets one run per surface — separate personas, separate rants. Test what a real user gets (deployed app, installed release, published package); a URL or install command from the user beats hunting for one.

Phase complete when the playbook is read and the thing-a-real-user-gets is pinned down (URL, install command, or package name).

## Phase 1 — The persona

Check `ai-docs/ux-personas/` first. A card for this product already exists → reuse it; this run becomes a regression pass against its ledger. Otherwise build one from the card template in `references/persona-gallery.md` — five questions plus the **runs ledger**, with seeds per surface and a worked example.

Phase complete when the card exists at `ai-docs/ux-personas/<slug>.md` and answers all five questions with answers concrete enough to quote — a name, an age, a tool, a breaking point in the surface's effort unit. "A user who dislikes the product" fails that bar.

## Phase 2 — Drive in character

Cold start: begin from nothing, per the playbook's recipe (new account, scratch config, empty project) — tag every account, key, and artifact you create with a `uxtest-` marker for cleanup. The empty first-run experience is where most friction lives; a pre-seeded setup skips exactly the screens that matter.

Attempt the persona's ONE task, start to finish — a goal-driven run, so every detour the surface forces on you is a finding in itself. **Count every unit of effort** (the playbook's unit) on the path to task completion. Along the way, visit each friction category in the playbook. If a locked-out state exists (paywall, expired key, rate limit), probe it: what happens to their data and work in flight when access ends?

Per pain point: capture the playbook's artifact → `ai-docs/ux-test/<slug>/<NN>-<desc>.<ext>` → `Read` it back.

Phase complete when the ONE task is done or abandoned-in-character, every friction category has been visited, the effort count is recorded, and every complaint has an artifact you read back.

## Phase 3 — The rant

Write the persona's review, fully in voice, to `ai-docs/ux-test/<slug>/rant.md`:

```markdown
# <PERSONA>'s review of <PRODUCT>

Overall: <keep using it? Yes / No / Maybe, with conditions>

THE GOOD (grudging admissions)
THE BAD (would stop them using it)
THE UGLY (would make them quit on the spot)

SPECIFIC COMPLAINTS

1. <place/feature>: "<quote in persona voice>" — what happened vs what they expected — <artifact>

VERDICT: "<one line, in voice>"
```

Phase complete when every Phase 2 pain point appears in the rant with its artifact linked.

## Phase 4 — The pragmatism filter

**Break character here.** As a product person, give every rant line exactly one color:

- **RED — real UX bug.** Any user hits this, not just grumpy ones. A competent-but-busy user would have the same complaint; genuine accessibility issues; exceeding the playbook's RED ceiling is RED regardless of persona.
- **YELLOW — valid, edge users only.** Real, but fixing it for everyone adds little.
- **WHITE — persona noise.** "I want it to work like my old way"; fixing it would add complexity for the 80% who are fine.
- **GREEN — feature request** hiding inside a complaint, often a missing onboarding moment.

Apply the playbook's perception caveat: complaints you inferred rather than experienced stay _inferred_ until the artifact confirms them — no RED on an inferred complaint.

Two calibration gates, both checked before moving on:

- **Zero complaints → the persona was too forgiving.** Make them less patient, more set in their old ways, and redo Phase 2.
- **Zero WHITE → the product has real problems, not a grumpy persona.** Say exactly that in the report.

Phase complete when every complaint carries one color and both gates have been checked.

## Phase 5 — Ledger, cleanup, report

Append this run's row to the persona card's runs ledger. On a rerun, diff against the previous entry and call out regressions ("core task cost 4 clicks in June, now 7") — these outrank any new finding.

Cleanup before reporting: delete the `uxtest-`-tagged accounts, keys, and data where the product allows it; where it doesn't, note what was left behind for the report. Keep `ai-docs/ux-test/` — that's the evidence trail.

Final message: the rant (visceral), then the filtered table (color, complaint, artifact, suggested fix), then the ledger diff, then anything cleanup left behind. Offer tickets via `AskUserQuestion` — if yes, file RED and GREEN items through the `jira` skill (persona quote + the objective issue underneath + suggested fix, max 10), and YELLOW as one catch-all. WHITE stays in the report only.
