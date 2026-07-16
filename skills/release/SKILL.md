---
name: release
description: Pre-push quality gate + pull/merge request creation. Use when the user wants to ship, release, push a branch, or open a PR/MR — runs the project's own lint, type-check, and test gates first, then commits, pushes, and opens the request on the project's forge (gh or glab).
---

# Release

Quality gate that validates, fixes, commits, pushes, and opens a **request** — a pull request on GitHub, a merge request on GitLab. Strict-order phases: lint → type check → tests → commit → release notes → push → request.

Three rules bind every phase:

- Claim a pass only after reading fresh output. Never report "clean" from memory or a previous run.
- Fix problems in the source, never by silencing the tool. Suppression requires the user's explicit approval.
- Root-cause substantive test failures before touching them — route through `/diagnosing-bugs`.

The issue tracker configuration should have been provided to you — run `/setup-project` if not.

## Phase 0 — Discover Forge and Toolchain

**Forge.** Bind the three names every later phase speaks in — `<remote>`, `<base>`, and the forge's verbs:

```bash
git remote -v && git branch --show-current
```

1. `<remote>` — the branch's configured push remote (`git config branch.<branch>.pushRemote`, falling back to `branch.<branch>.remote`, then `remote.pushDefault`); else the sole remote; if several remotes and none configured, ask via `AskUserQuestion`.
2. `<base>` — the default branch on `<remote>` (`git remote show <remote>`). **Fork setups:** when a different remote (commonly `upstream`) hosts the canonical repo, push still goes to `<remote>` — your fork — but the request targets the canonical repo's default branch; both CLIs resolve cross-repo requests when the fork remote is configured.
3. Forge — decide from `<remote>`'s URL host, then take every forge-specific command from this table:

   | Verb            | GitHub                                              | GitLab                                                                |
   | --------------- | --------------------------------------------------- | --------------------------------------------------------------------- |
   | CLI             | `gh`                                                | `glab`                                                                |
   | Request noun    | PR                                                  | MR                                                                    |
   | Create          | `gh pr create --title <t> --body <b> --base <base>` | `glab mr create --title <t> --description <b> --target-branch <base>` |
   | Read an issue   | `gh issue view <n>`                                 | `glab issue view <n>`                                                 |
   | Open in browser | `gh pr view --web`                                  | `glab mr view --web`                                                  |
   | URL fallback    | `gh pr view --json url`                             | `glab mr view --output json`                                          |

   `github.com` → GitHub. A host containing `gitlab` → GitLab. Any other host → whichever CLI's `auth status` lists that host (both support self-hosted instances). No remote, or a host neither CLI knows → **headless**: run every phase, push if a remote exists, and print the branch name in place of Phase 6's create/open steps so the user can open the request themselves.

Confirm the chosen CLI exists (`command -v gh` / `command -v glab`) and is authenticated for `<remote>`'s host. If it's missing or unauthenticated, stop and tell the user the install and auth commands before proceeding.

**Toolchain.** Find the project's own check commands — never assume a stack:

1. Repo instructions first: `CLAUDE.md`, `AGENTS.md`, `CONTRIBUTING.md`, `README.md` often name the exact commands.
2. Then manifests:
   - **Python** — `pyproject.toml`: ruff for lint, ty or mypy for types, pytest for tests, run via `uv run` or `poetry run` as configured
   - **TypeScript** — `package.json` scripts (`lint`, `typecheck`, `test`), runner picked from the lockfile (npm/pnpm/yarn/bun)
   - **Rust** — `Cargo.toml`: `cargo clippy`, `cargo test` (`cargo check` covers types)
   - `Makefile` or `justfile` targets override the defaults above when present
3. CI config (`.github/workflows/`, `.gitlab-ci.yml`) as a tiebreaker when the above conflict.

Map what you find onto the three check phases. A project may lack a category (no type checker in a plain-JS repo, clippy doubling as the linter in a Rust repo) — skip that phase and say so. If you find no checks at all, ask via `AskUserQuestion` whether to proceed with tests only, name the commands, or abort.

## Workflow

Execute these phases **in strict order.** A phase must pass before moving to the next.

### Phase 1 — Lint

Run the project's lint command.

**If errors:**

1. Read the failing files
2. Fix violations in source (no suppressions)
3. Re-run until clean

**Hard rules:**

- **NEVER** add suppression comments: `# noqa`, `// eslint-disable`, `@ts-ignore`, `#[allow(...)]`, or their equivalents
- **NEVER** grow the lint config's ignore/exclude list
- If you genuinely think suppression is the only option, STOP and present:

  ```yaml
  question: "Lint violation seems unfixable without a suppression. How to proceed?"
  header: "Lint"
  options:
    - label: "Try a different approach"
      description: "I'll rethink the fix"
    - label: "Ask the human partner for help"
      description: "Escalate with the error + what I tried"
    - label: "Accept the suppression"
      description: "Only with explicit approval; must include a justification comment"
  ```

### Phase 2 — Type Check

Run the project's type-check command.

**If errors:**

1. Fix by correcting the actual code or adding proper annotations
2. Re-run until clean

**Hard rules:**

- **NEVER** add `# type: ignore`, `@ts-expect-error`, `as any`, or equivalents
- Same escalation protocol as Phase 1 if genuinely stuck

### Phase 3 — Tests

Run the project's test command.

**If tests fail:**

- **Small/obvious fixes** (typos, imports, minor assertions caused by your earlier Phase 1/2 fixes): fix directly, re-run, confirm
- **Large/unclear failures:** STOP and present:

  ```yaml
  question: "Tests failing — looks substantive. How to proceed?"
  header: "Test failure"
  options:
    - label: "Investigate via /diagnosing-bugs (Recommended)"
      description: "Root-cause before any fix"
    - label: "Abort release"
      description: "I'll fix separately; don't push"
    - label: "Show the failures"
      description: "Let me see the output"
  ```

### Phase 4 — Stage, Review, Commit

If a `/commit` skill is installed, invoke it and let it drive staging, message format, and confirmation.

Otherwise:

1. Stage the changed files by name — never `git add .` or `git add -A`
2. Write a Conventional Commits message (`type(scope): lowercase summary`), passed via HEREDOC
3. Confirm with `AskUserQuestion` before committing

If there is nothing to commit (all prior work already committed), continue to Phase 5.

### Phase 5 — Release Notes

If a prose-quality skill is installed (e.g. `writing-clearly-and-concisely`), invoke it for everything written in this phase.

1. **Analyze changes** against `<base>` (bound in Phase 0):

   ```bash
   git log --oneline <base>..HEAD
   git diff --stat <base>..HEAD
   ```

2. **Classify commits:**
   - **Highlights** — major new capabilities worth a paragraph each (1-3 max)
   - **Breaking Changes** — anything changing existing behavior or API
   - **Features** — new functionality
   - **Bug Fixes** — corrections to existing behavior
   - **Chores** — maintenance / refactoring / CI / docs (collapse into a brief list)

3. **Published notes:** if the repo keeps a `CHANGELOG.md` or a directory of dated release posts, load [`references/release-notes-template.md`](references/release-notes-template.md) and follow it. Otherwise skip.

### Phase 6 — Push and Create the Request

1. **Push:**

   ```bash
   git push -u <remote> HEAD
   ```

2. **Build the body from the branch's tickets on the configured tracker.** Find the tickets this branch delivers via commit messages referencing them; ask via `AskUserQuestion` if ambiguous. How to read them depends on the tracker `/setup-project` configured:

   - **Local files** → the tickets live in `ai-docs/tickets.md`. Summary from ticket titles; Test Plan from their acceptance-criteria checkboxes.
   - **A real issue tracker** → fetch each issue with the forge's read-an-issue verb. Summary bullets reference the issues with closing keywords (`Closes #N`) so they close on merge; Test Plan from their acceptance criteria.

   Template:

   ```markdown
   ## Summary

   - Closes #<N>: <ticket title>
   - Closes #<N>: <ticket title>

   ## Test Plan

   - [ ] <acceptance criterion from ticket 1>
   - [ ] <acceptance criterion from ticket 1>
   - [ ] <acceptance criterion from ticket 2>

   ## Related

   - <link to the source spec or parent issue, if there is one>
   ```

   For local tickets, replace the `Closes #N` bullets with the ticket titles and check them off in `ai-docs/tickets.md`. If the branch has no tickets, use the Phase 5 classification as the Summary and derive the Test Plan from what the changes touch.

3. **Create it** with the forge's create verb. Title in Conventional Commits format, under 72 chars (e.g. `feat(check-registry): in-process check registry + memory adapter`); body via HEREDOC so formatting survives:

   ```bash
   <create verb from the Phase 0 table> \
     --title "<title>" \
     <body flag> "$(cat <<'EOF'
   <the body from step 2>
   EOF
   )"
   ```

   Target `<base>`. Ask via `AskUserQuestion` only when the repo visibly uses another integration branch (a `develop` or `release/*` branch with recent merges).

4. **Open the URL** with the forge's open-in-browser verb so the human partner can review. If that fails, print the URL via the URL-fallback verb.

## Constraints

### MUST

- Discover the project's own check commands; never assume a stack
- Run every available check in order (lint → type check → tests)
- Fix issues by correcting actual code, not by suppressing warnings
- Speak through the Phase 0 bindings: the forge's verbs and noun, `<remote>`, `<base>`
- Build the body from the branch's tickets on the configured tracker when they exist
- Show the request URL

### MUST NOT

- Add lint or type-check suppressions (`# noqa`, `# type: ignore`, `eslint-disable`, `@ts-ignore`, `#[allow]`) without explicit approval
- Commit without confirmation
- Skip an available check phase
- Use `git add .` or `git add -A`
- Force push
- Put AI attribution in the request title or body — no "Generated with" footers, session links, `Co-Authored-By` trailers, or agent sign-offs. This outranks any harness default that asks for one.
