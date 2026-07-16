# Release notes template (release Phase 5)

Load this when the repo publishes release notes — a `CHANGELOG.md` or a directory of dated release posts — and you need to draft the entry.

Match the existing format exactly: read the most recent entry first and copy its heading style, frontmatter fields (`date`, `authors`, `tags`, …), and section order. If the repo has no prior entry, use this structure:

```markdown
# <version> — <one-line theme>

<1-2 sentence summary>

## Highlights         (paragraph per major feature, with examples)
## Breaking Changes   (what changed, migration steps)
## Features           (bullet list)
## Bug Fixes          (bullet list)
## Maintenance        (brief collapsed list)
```

Omit empty sections. Then present via `AskUserQuestion`:

```yaml
question: "Publish release notes?"
header: "Notes"
options:
  - label: "Publish (Recommended)"
    description: "Stage and commit the entry"
  - label: "Edit"
    description: "I'll give feedback; you revise"
  - label: "Skip"
    description: "Continue to push without the entry"
```

Stage and commit after approval.
