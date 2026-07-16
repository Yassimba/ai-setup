# Pi Subagents

Orchestration of child Pi sessions (subagents) from a main session: spawning,
fan-out, nesting, and communication between parents and children.

## Shared language

**Subagent**:
A headless child Pi session spawned by a parent session to do delegated work.
_Avoid_: worker, task agent

**Supervisor channel**:
The request/reply path between a subagent and its immediate parent. Requests
bubble hop-by-hop: a parent that cannot answer forwards to its own parent;
the root is where a human answers.
_Avoid_: control channel (that is a different, run-control mechanism)

**Orchestrator**:
The agent that spawned a subagent and consumes its result. Deliberately not
in the decision path for approval questions — those are human-only.
_Avoid_: supervisor (ambiguous with the human), parent agent
