# REST / HTTP API playbook

The worst-case persona spans a wider range here than for a library — from deadline dev to barely-technical low-code integrator; the gallery seeds both.

**Drive:** consume it as shipped, over HTTP, with `curl` from the terminal — the docs are the UI. Start where a real integrator starts: docs landing page, sign-up, key acquisition, first authenticated request. If key acquisition routes through a web dashboard, friction on that leg still counts — capture screenshots there (the web playbook's artifact) and keep going.

**Evidence:** request + response transcript per pain point, saved to `<NN>-<desc>.txt` with credentials stripped.

**Effort unit:** failed requests + doc lookups. **RED ceiling:** >3 failed requests to the first authenticated 200, or a documented example that doesn't run verbatim.

**Cold start:** fresh account and key — the key-acquisition path IS the first impression.

**Friction categories:**

- **First impression** — docs landing to first authenticated 200: how far?
- **Error recovery** — wrong field, missing param, bad auth on purpose. Does the error body name the field and the fix, or just echo "400 Bad Request"?
- **Copy-paste fidelity** — run the docs' own examples verbatim; every one that fails is a finding
- **Terminology** — one concept, one name, across endpoints, docs, and error bodies
- **Consistency** — pagination, casing, id formats, plural/singular: does learning one endpoint teach the next?
- **Discoverability** — does the persona's first guess in the reference/OpenAPI search land on the right endpoint?
- **Surprise** — silent rate limits, undocumented defaults, versioning that bites, side effects where none belong

**Locked-out probe:** rate limit, revoked key, exhausted quota.

**Perception caveat:** you never sat through real network latency or a real sign-up flow (captchas, email verification) — mark speed and sign-up complaints _inferred_ unless the transcript shows them.
