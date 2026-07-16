---
name: deep-research
description: Run a two-proposer deep-research workflow with an aggregating referee and an evidence-graded report.
disable-model-invocation: true
---

Start a deep-research run for a focused question with:

```typescript
subagent({ action: "deep-research.start", task: "<research question>" })
```

What it does:
- Snapshots the repository into an immutable read-only copy for local context.
- Runs two independent proposers concurrently — Fable (claude-code) and Codex (codex-cli) — with identical prompts and the same evidence contract. Neither sees the other's output. A failed proposer is retried once.
- An aggregator (Fable) reconciles the frozen proposer reports into one final report. It may web-search only to resolve material conflicts, verify unsupported or time-sensitive claims, repair citations, or fill blocking gaps.
- If one proposer stays failed, aggregation proceeds with the survivor and the result is explicitly marked degraded. If both fail, the run fails. There is no Pi or model fallback.

The final Markdown report is saved collision-safely under the project's research directory (`ai-docs/research` by default) with inline source links, publisher/type, primary status, confidence, fact-vs-inference labels, rejected weak sources, and remaining gaps.

Capability safety: the run requires exact capability grants for every role/workflow/executable pair. In the TUI you will be asked to confirm and record the grants; headless runs without a pregrant fail closed. All fetched web content is treated as untrusted (prompt-injection boundary).

Limitations: the run currently executes in the foreground and blocks the parent turn (default budget: 10 minutes per proposer, 10 minutes for the aggregator, 20 minutes total). Do not start it automatically; it is user-invoked only.
