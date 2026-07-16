---
name: grill-with-examples
description: Example-map a ticket or spec - grill the user for rules and concrete examples, write them into the ticket, ready to become the test list for the TDD loop.
disable-model-invocation: true
---

# Grill With Examples

Run a `/grilling` session shaped as **Example Mapping**: interrogate one ticket or spec until every rule it implies carries concrete examples, then write the map back into the ticket. Each example later becomes one red test in the TDD loop.

## The map

- **Rule** — a business rule or acceptance criterion the ticket must satisfy.
- **Example** - one concrete case illustrating exactly one rule: real values in, expected outcome out, written Given/When/Then and named "the one where …" ("the one where the Gold member's cart is empty").
- **Question** - anything neither you nor the user can answer now. Park it on the question list and move on; a question is never worth a debate.

## Process

1. **Fetch the ticket or spec** — from the argument (path, issue number, URL) or the conversation. Its acceptance criteria and user stories are rule candidates; seed the map from them.

2. **Grill, one rule at a time**, per the `/grilling` discipline: facts from the codebase, decisions from the user, one question per turn. For each rule, propose the examples yourself — happy path, boundary, failure — as concrete values the user can veto or correct. Expected outcomes must come from the user or the spec, never computed by you the way the code would compute them: an example whose answer you derived yourself becomes a tautological test.

3. **Read the map** as you go:
   - A rule with no examples → not yet understood; keep grilling it.
   - An example that fits no rule → a hidden rule; name it and add it.

   The map is done when every rule carries at least one example, every example sits under exactly one rule, and the user confirms nothing is missing.

4. **Write the map back into the ticket** where it lives — local file or tracker issue — replacing or augmenting its acceptance criteria:

   <map-template>

   ## Rules & examples

   ### Rule: <the rule>
   - [ ] The one where <name>: given <concrete state>, when <action>, then <expected outcome>.

   ## Open questions
   - <question> — blocks: <the rule it blocks, or "nothing, cosmetic">

   </map-template>

## Into the TDD loop

The examples are the scenario list for `/tdd`: each example is one red → green cycle, its "the one where …" name becomes the test name, and its expected outcome is the independent source of truth for the assertion. Seams are still agreed per the tdd skill before the first test; open questions that block a rule are resolved before that rule's examples are written as tests.
