# Web UI playbook

**Drive:** browser mechanics come from the `browser-usage` skill. Test the deployed or staging app.

**Evidence:** screenshot per pain point (`.png`). Additionally run `agent-browser console` and `agent-browser errors` on every page — silent JS errors are the highest-value finds.

**Effort unit:** clicks. **RED ceiling:** >5 clicks to the ONE task.

**Cold start:** register as a NEW user — never a pre-seeded admin account.

**Friction categories:**

- **First impression** — would they bother past the landing page?
- **Error recovery** — do something wrong on purpose; can they get back?
- **Readability** — text size, contrast, information density
- **Speed** — does it feel faster than their current method?
- **Terminology** — jargon they wouldn't know
- **Navigation** — do they know where they are? can they get back?

**Locked-out probe:** the paywall.

**Perception caveat:** you inferred readability from a DOM, not through 58-year-old eyes. Verify contrast/font-size complaints against the screenshot before granting RED.
