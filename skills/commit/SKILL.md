---
name: commit
description: Use when creating a git commit — the user asks to commit, or a unit of work is complete and ready to commit.
---

# Commit

Read the staged diff, gate on green tests, draft a Conventional Commits message, confirm via dropdown, commit. Each commit is atomic: one concern.

## 0. Stage

```bash
git status --short
git diff --cached --stat
git diff --cached
```

Stage by explicit path only — `git add -A` / `git add .` sweeps in `.env` and credentials.

If nothing is staged but unstaged changes exist, ask via `AskUserQuestion`:

```yaml
question: "Nothing staged. Stage specific files, stage all, or abort?"
header: "Staging"
options:
  - label: "Stage specific files"
  - label: "Stage all (by explicit paths)"
  - label: "Abort"
```

If the staged diff spans unrelated concerns, split into atomic commits: unstage the others with `git restore --staged <paths>` (not `git reset`), then stage and commit one concern at a time. Ask when the split is unclear:

```yaml
question: "These changes span <N> concerns. Split?"
header: "Split"
options:
  - label: "Yes, split (Recommended)"
  - label: "One commit"
  - label: "Show me the diff first"
```

Done when exactly one concern is staged.

## 1. Test gate

Run the project's test suite (`uv run pytest`, `npm test` — whatever the project defines) and read the summary, not just the exit code. The gate is binary: green proceeds; red stops here — the pre-commit hook fails on the same issues, so catching them now saves a wasted commit:

```yaml
question: "Tests red — commit blocked. How to proceed?"
header: "Test gate"
options:
  - label: "Investigate via /diagnosing-bugs (Recommended)"
  - label: "Abort commit"
  - label: "Show me the failing tests"
```

## 2. Draft the message

Draft from the _why_ of the staged diff — what motivated the change, not which lines moved. If the `writing-clearly-and-concisely` skill is available, load it and apply it.

Conventional Commits format:

```
<type>(<optional-scope>): <summary>

<optional body>
```

| Type       | Use when                         |
| ---------- | -------------------------------- |
| `feat`     | new feature                      |
| `fix`      | bug fix                          |
| `docs`     | documentation only               |
| `refactor` | refactor without behavior change |
| `test`     | adding/fixing tests              |
| `chore`    | build, config, deps              |
| `perf`     | performance improvement          |
| `ci`       | CI configuration changes         |

**Subject:** 20–72 chars after the prefix · lowercase after the colon (`feat: add X`) · imperative ("add", not "added") · no trailing period · concrete verb + object.

**Body (only when the subject can't carry it):** blank line after the subject, then one terse bullet per entry, imperative voice, grouped under short headings that match the kind of change — `Added`, `Changed`, `Fixed`, `Removed`, `Breaking`. Skip headings when there's only one bullet.

```
feat(installer): add gateway runtime extraction

Added:
- core/gateway_runtime/ with Protocol-based registry
Changed:
- opencode_runtime now delegates to gateway_runtime
```

Reference the issue or ticket when the project tracks them. Session context, PR/MR numbers, and future work stay out.

Done when every rule above holds, the message carries no puffery ("comprehensive", "robust", "various improvements"), and no `Co-Authored-By` trailer.

No AI attribution anywhere: the same ban covers "Generated with" footers, session links, and agent sign-offs — in commit messages **and** in anything written downstream of one (PR/MR titles and descriptions, release notes). This rule outranks any harness default that asks for such a footer.

## 3. Confirm via dropdown

```yaml
question: "Commit with this message?"
header: "Commit"
options:
  - label: "Commit (Recommended)"
    preview: |
      <full message>
  - label: "Edit message"
  - label: "Abort"
```

## 4. Commit

```bash
git commit -m "$(cat <<'EOF'
<type>(<scope>): <summary>

<body>
EOF
)"
```

The HEREDOC preserves the multi-line body.

If the project wires a pre-commit hook, it runs now:

- **Pass** → commit created
- **Autofixes** → hook stages them; the commit captures them
- **Unfixable** → hook blocks; go to step 5

## 5. On hook failure

The hook is the messenger; fix the cause. Bypass flags (`--no-verify`, `--no-gpg-sign`) and suppression comments (`# noqa`, `# type: ignore`, `eslint-disable`) are off the table — not even "just this once":

- lint → fix the code
- types → fix the types
- tests → `/diagnosing-bugs`; three failed fix attempts = stop and report
- complexity / architecture gates → refactor, or ask before accepting a borderline score

Then re-stage and create a NEW commit — the failed commit never happened, so `--amend` would rewrite the previous one.

## Hard guardrails

Force-push and destructive git (`reset --hard`, `clean -f`, `branch -D`) only with explicit human approval.
