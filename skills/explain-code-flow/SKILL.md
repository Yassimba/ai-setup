---
name: explain-code-flow
description: Generate a visual architecture walkthrough for a feature in the current codebase. Use when the user asks how a feature works at a high level — "explain X", "how does X work", "trace the X flow".
---

# Explain Code Flow

Turn a feature in the current codebase into a readable architecture walkthrough, rich with ASCII diagrams and `file:line` anchors, so the reader can navigate from the high level down to the exact place where something happens.

## When to use

Use this skill when the user wants orientation, not answers. Signs:

- They say "explain", "walk me through", "how does X work", "trace X", "give me the big picture", "architecture of X".
- They are onboarding, reviewing a subsystem, or trying to build a mental model before making changes.
- They are not debugging a specific error and not asking about one function.

If the user is debugging or asking a narrow question, answer it directly instead. This skill is overkill for those.

## The core idea

A good code-flow explanation answers three questions in order:

1. **Where does this feature live?** The 30,000ft view. Entry point, top-level layers, external systems.
2. **What are the building blocks?** Key abstractions, protocols, types. Anchored to `file:line`.
3. **What actually happens at runtime?** A per-request trace showing who calls whom, where data transforms, where state is read or written.

## Required workflow

Five phases.

### Phase 1. Parse the target and orient briefly

Read the user's request and extract the target feature. It is usually a phrase like "the dashboard", "quality checking", "the LSP server", "row flagging". If ambiguous, ask one clarifying question and stop. Do not guess.

Then do *just enough* to know where to send the subagent: one or two searches to locate candidate directories. A single `Glob` on the feature keyword is usually enough. If the user named a symbol, use `LSP workspaceSymbol` to jump to its definition and use that path as the orientation anchor.

**Do NOT start reading source files in the main context.** Orientation is pointing a finger; Phase 2 does the actual mapping.

### Phase 2. Delegate the mapping to an Explore subagent

Spawn an Explore subagent (`subagent_type: "Explore"`, thoroughness `"very thorough"`) with the template prompt below. This is mandatory, not advisory.

```
I need to explain how <FEATURE> works in <PROJECT ROOT>. Map it with enough
detail that I can turn this into an architecture walkthrough with ASCII
diagrams.

For each of the following, give me: exact file paths, line ranges, and 2-3
lines of surrounding context. Report in well-organized prose, under 1500
words. Focus on call chains over prose.

1. Entry point(s). What launches or triggers this feature? How does it hand
   off to the rest of the code?

2. Composition root / main wiring. Where does the feature assemble its
   dependencies? Walk through the order: config, adapters, registry,
   handlers.

3. Core types / domain model. What are the central abstractions? List them
   with file:line. Include Protocols, dataclasses, enums that matter.

4. Ports and adapters. If this feature uses protocols with multiple
   implementations, list them and where each one lives.

5. Per-request / per-event flow. Trace a single call from the top all the
   way down to where real work happens. Include every hop with file:line.

6. Mode variations. Are there swappable behaviors (demo vs prod, mock vs
   real, feature flags)?

7. Catalogs. Are there many similar things (pages, endpoints, handlers,
   components)? List them with a one-line purpose each.

8. Approximate file count and total line count for the feature (so the
   caller can pick the right sizing bucket).

Use LSP when it beats grep:
- goToImplementation for protocol implementations
- incomingCalls / outgoingCalls for call hierarchy
- workspaceSymbol for quick symbol lookup
- hover for type signatures
```

### Phase 3. Verify the call chain with LSP, then size

Before drawing the per-request flow diagram, verify each hop. The Explore report is usually right on structure but can miss a hop in the per-request flow.

Required LSP checks (call-chain only — file:line anchors elsewhere can stay as Explore reported):

1. For every Protocol or ABC referenced in the flow, call `LSP goToImplementation` at its definition to confirm the list of concrete implementations matches what Explore reported.
2. For the top function in the flow (the entry called by the user), call `LSP incomingCalls` to confirm no unexpected callers.
3. For any function you draw as a hop in the flow, call `LSP outgoingCalls` to confirm the called function exists and check its signature. If the hop is wrong, fix the diagram before writing the walkthrough.
4. For any concrete type name you intend to put in the diagram, call `LSP hover` at a reference site to confirm the real type, in case it was aliased or re-exported.

Then size by counting (don't eyeball it — use the file count from the Explore report):

| Files involved | Bucket | Word target  | Sections                                           |
|----------------|--------|--------------|----------------------------------------------------|
| 1-3            | Small  | 200-400      | 30,000ft + key abstractions (3-5) + flow diagram   |
| 4-10           | Medium | 500-900      | Above, plus 1-2 conditional sections that apply    |
| 11+            | Large  | 1000-1500    | Above, plus layers diagram and grouped abstractions|

If your first instinct and the count disagree, trust the count.

### Phase 4. Write the walkthrough

Follow the output template below. Keep diagrams ASCII. Use box-drawing characters (`┌ ┐ └ ┘ │ ─ ├ ┤ ┬ ┴ ┼ ▼ ▲ ◄ ►`). Use backtick code fences for diagrams so they render monospaced.

Anchor every structural claim to `file:line`. The reader should be able to click-navigate from any statement to the line that proves it.

### Phase 5. Polish, save, present

Once the walkthrough draft exists:

1. **Polish prose**: invoke the `writing-clearly-and-concisely` skill via the Skill tool and apply its revisions. Pass the full walkthrough; the writing skill leaves diagrams and tables alone.
2. **Save** to a markdown file. Default path: `ai-docs/explanations/<feature-slug>.md` (create dirs if needed). Slug is lowercase-kebab-case of the feature name.
3. **Print** the polished walkthrough to the conversation and tell the user the file path.

## Output template

The walkthrough has required and conditional sections. Scale the required ones with feature size (see Phase 3).

### Required sections

**1. Opening line.** One sentence naming the feature and where it lives. No fluff.

**2. 30,000ft view.** One ASCII diagram showing the outermost black-box view:
- Entry point (CLI command, HTTP request, cron, user action)
- Top-level components the request passes through
- External systems touched (DB, API, filesystem)
- Exit point (response, side effect, file)

Example shape:

````
┌────────────────────────────────────────────────────────┐
│    $ <entry command>                                   │
│                    │                                   │
│                    ▼                                   │
│    ┌──────────────┐      ┌──────────────┐              │
│    │ <composition │◄────►│ <external    │              │
│    │  root>       │ HTTP │  service>    │              │
│    └──────────────┘      └──────────────┘              │
│                    │                                   │
│                    ▼                                   │
│    <output / observable effect>                        │
└────────────────────────────────────────────────────────┘
````

**3. Key abstractions.** A short list, each with a `file:line` anchor and one sentence describing its role. Target 3-7 entries for small/medium. If there are more, the feature is large and you need to group them (see "Large features" below).

**4. Per-request (or per-event) flow.** The centerpiece. One ASCII diagram tracing a single call through the system, top to bottom. Every step has a `file:line` anchor on the right. Show where data transforms, where state is read or written, where control passes between layers.

Example shape:

````
Entry invoked                             <file>:<line>
          │
          ▼
  <step 1: parse / resolve>               <file>:<line>
          │
          │ produces <intermediate type>
          ▼
  <step 2: dispatch>                      <file>:<line>
          │
          │ ┌──────────────────────────────────────┐
          │ │ singledispatch on <type>             │
          │ │                                      │
          │ │ if <cond>: <handler A>               │
          │ │ elif <cond>: <handler B>             │
          │ └──────────────────────────────────────┘
          ▼
  <step 3: IO>                            <file>:<line>
          │
          ▼
  <step 4: transform response>            <file>:<line>
          │
          ▼
  Return to caller
````

### Conditional sections

Include these only when the feature has the structure. Skipping an inapplicable section is better than forcing it.

**A. Layers diagram.** Include when the feature spans multiple architectural layers (hexagonal, layered, MVC). Show which module depends on which, with arrows. Required for Large features.

**B. Routing / dispatch table.** Include when there is typed dispatch: `singledispatch`, a `match` on type, a registry dict, URL routing. A table mapping input type to handler to endpoint beats prose.

Example shape:

````
┌───────────────────────┬──────────────────────────────┐
│ Input                 │ Handler / Destination        │
├───────────────────────┼──────────────────────────────┤
│ <resource / route>    │ <handler fn>  (<file>:<line>)│
│ ...                   │ ...                          │
└───────────────────────┴──────────────────────────────┘
````

**C. Component / feature catalog.** Include when there are many similar things (pages, endpoints, components, plugins). Table them, one line per entry.

**D. Mode variations.** Include when the feature has swappable behavior (demo vs prod, mock vs real, flag-gated). Show the fork as an ASCII diagram or bullet list.

**E. DI / ambient context.** Include when the feature uses non-trivial dependency injection, context variables, or middleware.

**F. Closing paragraph.** One short paragraph summarizing the shape. Three sentences max. Skip for Small features.

## Large features: grouping the abstractions

When there are more than 7 key abstractions, group them by architectural layer or bounded context, not alphabetically:

````
Core        │ <type A>, <type B>, <type C>
Adapters    │ <impl X>, <impl Y>
Presentation│ <view P>, <view Q>
````

## Style rules

- **ASCII diagrams only.** No mermaid, no images, no SVG. The output must render in a terminal.
- **Anchor everything.** Every structural claim gets a `file:line`. No exceptions.
- **Concrete over abstract.** Prefer the real name of the function over a generic label. `ApiQueryDispatcher.execute()` beats `the dispatcher`.
- **Short prose.** Tables and diagrams carry the information. Prose exists to connect them.
- **No em-dashes.** Use commas, parentheses, or sentence breaks.
- **Present tense, active voice.** "Resource knows its return type" beats "the return type is known by Resource".

## Example

**User:** "explain how the dashboard flow works"

Phase 1: target is "dashboard"; `Glob **/dashboard/**` locates `src/<project>/presentation/dashboard/`, `src/<project>/core/dashboard/`, `src/<project>/adapters/dashboard/`. Phase 2: dispatch Explore subagent with the template. Phase 3: verify the dispatcher protocol's implementations with `LSP goToImplementation`; verify the per-page flow with `incomingCalls` on the page-wrapping entry function; report says 30+ files, bucket is Large. Phase 4: write with layers diagram, routing table, mode variations. Phase 5: chain writing-clearly-and-concisely; save to `ai-docs/explanations/dashboard.md` and print.
