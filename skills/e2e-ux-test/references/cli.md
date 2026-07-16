# CLI playbook

**Drive:** run the installed binary via Bash, exactly as a user gets it (released install path, not the dev checkout). If the tool is an interactive TUI you cannot drive, say so and stop — don't fake a session.

**Evidence:** terminal transcript per pain point — the command plus its full output saved to `<NN>-<desc>.txt`.

**Effort unit:** commands typed + doc lookups (`--help`, `man`, web). **RED ceiling:** >3 doc lookups to finish the ONE task, or a failure that exits 0.

**Cold start:** scratch `HOME`/`XDG_CONFIG_HOME` so no leftover config exists — the very first invocation is the first impression.

**Friction categories:**

- **First impression** — bare invocation and `--help`: is the first thing to type obvious?
- **Error recovery** — misuse it on purpose: typo a flag, feed bad input, run steps out of order. Does the error say what to do *next*, or just what broke?
- **Readability** — help walls, output density, color as signal vs decoration, does it survive an 80-column terminal?
- **Speed** — startup latency; progress feedback on anything long
- **Terminology** — flag names and jargon; consistency (`--dry-run` here shouldn't be `-n` there)
- **Discoverability** — can they find the right subcommand from `help` alone? examples in the help? completions?

**Locked-out probe:** no network, no permissions, expired license.

**Perception caveat:** you parse output as tokens; a human scans it visually. Re-read the saved transcript as a rendered block before granting a readability RED.
