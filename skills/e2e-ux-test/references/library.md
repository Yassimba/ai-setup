# Code API / library / SDK playbook

**Drive:** create a scratch project (empty venv / fresh `package.json` / new module) and integrate for real: install the published package, reach hello-world, then attempt the ONE task. Keep every failed attempt as its own file (`attempt-01.py`, `attempt-02.py`, …), never overwrite — the pile of attempts IS the finding.

**Evidence:** the attempt file plus its captured output/error per pain point.

**Effort unit:** wrong attempts + doc lookups. **RED ceiling:** >3 wrong attempts to hello-world, or the ONE task forcing you to read implementation source — a "surface only" breach the product caused.

**Cold start:** empty project, no prior config or credentials.

**Friction categories:**

- **First impression** — README/quickstart: how far is hello-world?
- **Error recovery** — pass wrong types, wrong order, missing config on purpose. Does the error name the fix, or just the symptom? Caught at compile time or only at runtime?
- **Copy-paste fidelity** — run the README/quickstart examples verbatim; every one that fails is a finding
- **Signature legibility** — naming, argument counts, bare booleans; what a reader infers without opening docs
- **Terminology** — jargon in public names; one concept, one name, everywhere
- **Discoverability** — would the persona's first guess (autocomplete, docs search) find the entry point?
- **Surprise** — defaults that bite, hidden state, required call order, silent retries or mutations

**Locked-out probe:** rate limit, expired key, quota exhausted.

**Perception caveat:** you never felt real IDE autocomplete or real latency — mark discoverability and speed complaints _inferred_ unless an artifact shows them.
