---
name: fable-advisor
description: Activate Fable as a session-scoped strategic advisor.
disable-model-invocation: true
---

Activate the Fable advisor for this persisted Pi session with:

```typescript
subagent({ action: "advisor.activate" })
```

After activation, Pi may call `subagent({ action: "advisor.ask", message: "<focused question>" })` after orientation, when stuck or considering a pivot, and before completing complex work. Skip consultation for short mechanical tasks. The advisor is tool-free; when it returns `need_evidence`, gather the exact requested evidence and consult again with the result.

Use `advisor.status`, `advisor.reset`, or `advisor.disable` for lifecycle management. Do not activate this skill automatically; it is user-invoked only.
