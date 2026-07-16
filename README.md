# ai-setup

Everything a coding-agent setup needs, in one repo you can install from: 50+ skills, Herdr plugins, and Pi packages. A guided installer walks you through the collection, sets up anything you're missing (like Pi or Herdr), and checks that it all works.

## What's inside

**Agent skills** — 50+ skills for coding agents that read a `skills/` tree (Claude Code included). They're about how you work with an agent, not what it builds: test-driven development, code review, refactoring, debugging, domain modeling, docs and diagrams, planning and backlog flow. Install one or all of them.

**Herdr plugins** — plugins for [Herdr](https://herdr.dev), a terminal multiplexer for coding agents. They ship as prebuilt, checksum-verified binaries, so you don't need Rust installed:

- [**reviewr**](plugins/herdr-reviewr/) - Work together with your AI to review code: read its diff, comment on lines, send the comments back with one key. It also reviews GitHub PRs and GitLab MRs (checks, threads, grouped replies) without leaving the pane.
- [**project-switcher**](plugins/herdr-project-switcher/) — fuzzy project picker with a git-aware file tree and README preview.
- [**jumplist**](plugins/herdr-jumplist/) — jump back and forward through the panes you've focused, like editor navigation history.

**Pi packages** — extensions for the [Pi](https://github.com/badlogic/pi-mono) coding agent.

- [**openai-fast**](plugins/openai-fast/) — turn on OpenAI fast mode (the priority service tier) from inside Pi.
- [**herdr-worktree**](plugins/herdr-worktree/) — continue the current session in a fresh Herdr-managed git worktree.
- [**mindwalk**](plugins/mindwalk/) — run `/mindwalk` to replay the current session as light moving through a 3D map of the codebase.
- [**subagents**](plugins/pi-subagents/) — delegate tasks to subagents: chain them, run them in parallel, let them ask you questions mid-run.
- [**rewind**](plugins/rewind/) — records file checkpoints as you work; branch to an earlier message and it restores your files to match.
- [**web-access**](plugins/web-access/) — `web_search` and `fetch_content` tools for pages, PDFs, and GitHub repos.

## The engineering flow

Most of the skills chain into one loop: you start with a vague idea and end with committed code. Each step feeds the next.

1. [**brainstorming**](skills/brainstorming/SKILL.md) — no idea yet. Talk through directions until one sticks; you end with a short brief.
2. [**wayfinder**](skills/wayfinder/SKILL.md) — the idea is real but fuzzy, or too big for one session. Break the unknowns into investigation tickets and work through them until the route is clear.
3. [**grill-with-docs**](skills/grill-with-docs/SKILL.md) — the agent interviews you, hard, until the idea is a spec. ADRs and a glossary fall out for free.
4. [**grill-with-examples**](skills/grill-with-examples/SKILL.md) — for hairy business logic: pin down every rule with concrete examples. Each example becomes a test later.
5. [**to-spec**](skills/to-spec/SKILL.md) — turn the conversation into a spec on your tracker. No more questions, just writing it down.
6. [**to-tickets**](skills/to-tickets/SKILL.md) — split the spec into small tickets that each say what blocks them.
7. [**implement**](skills/implement/SKILL.md) — build it. You approve a design sketch first, then it's TDD from there.
8. [**code-review**](skills/code-review/SKILL.md) — review the diff twice over: does it follow the repo's rules, and does it do what the spec said.
9. [**refactor**](skills/refactor/SKILL.md) — clean up, with the goal that the codebase ends up smaller than it started.
10. [**e2e-test**](skills/e2e-test/SKILL.md) — actually run the app and watch it work; a green test suite isn't proof. Add [**e2e-ux-test**](skills/e2e-ux-test/SKILL.md) when a human has to like the UI too.
11. [**commit**](skills/commit/SKILL.md) — tests green, then one clean Conventional Commits commit.

Two more sit off to the side. If something breaks mid-flow, [**diagnosing-bugs**](skills/diagnosing-bugs/SKILL.md) makes you find the actual cause before anyone touches a fix. And [**writing-clearly-and-concisely**](skills/writing-clearly-and-concisely/SKILL.md) keeps the prose the loop produces (specs, commit messages, this README) readable.

You don't have to do all of it. A small, clear feature can start at step 7.

## Guided setup

### macOS and Linux

```bash
curl -fsSL https://raw.githubusercontent.com/Yassimba/ai-setup/main/install.sh | sh
```

### Windows

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -Command "irm https://raw.githubusercontent.com/Yassimba/ai-setup/main/install.ps1 | iex"
```

Then run:

```bash
ai-setup setup
```

A wizard walks through **Pi packages**, then **Herdr plugins**, then **skills** by category, and shows you the exact install plan before it runs anything. Pi packages and Herdr plugins go through their own package managers; skills are copied straight into your agents' skill trees. It ends by telling you what to try first.

Come back to it later:

```bash
ai-setup add       # choose more capabilities
ai-setup update    # update installed tooling and resources
ai-setup doctor    # check the setup
ai-setup add --skill tdd --herdr-plugin reviewr --yes
```

There's also an [`ai-setup`](skills/ai-setup/SKILL.md) skill, so a coding agent can run this same setup for you and ask before each step.

## Install directly

You don't need the CLI — it just drives the tools below, and each one works on its own.

### Agent skills

The CLI installs skills natively: it detects which coding agents you have (`~/.claude`, `~/.agents`, `~/.codex`, `~/.pi/agent`) and copies the skills you pick — plus any skills they declare as dependencies — into each agent's skill tree.

```bash
ai-setup add --skill tdd --yes     # one skill (and whatever it depends on)
ai-setup update                    # refresh installed skills, backfill new agents
```

The repository is also on [skills.sh](https://skills.sh):

```bash
npx skills add Yassimba/ai-setup
```

Claude Code users can get the same skills from its marketplace:

```text
/plugin marketplace add Yassimba/ai-setup
/plugin install ai-setup@ai-setup
```

### Pi packages

Each Pi package installs on its own:

```bash
pi install npm:@yassimba/pi-herdr-worktree
pi install npm:@yassimba/pi-mindwalk
pi install npm:@yassimba/pi-openai-fast
pi install npm:@yassimba/pi-rewind
pi install npm:@yassimba/pi-subagents
pi install npm:@yassimba/pi-web-access
```

See the package README under [`plugins/`](plugins/) for its commands and configuration.

### Herdr plugins

Install any Herdr plugin straight from its folder in this repo:

```bash
herdr plugin install Yassimba/ai-setup/plugins/herdr-jumplist --yes
herdr plugin install Yassimba/ai-setup/plugins/herdr-project-switcher --yes
herdr plugin install Yassimba/ai-setup/plugins/herdr-reviewr --yes
```

Compiled plugins download checksum-verified binaries for macOS, Linux, or Windows from this repo's GitHub releases. No Rust needed.

## Repository layout

- `skills/<name>/SKILL.md` — the reviewed shared skills. Category grouping lives in `skills.sh.json`.
- `plugins/<name>/` — Pi packages and Herdr plugins, each installable on its own.
- `cli/ai-setup/` — the Rust setup CLI.
- `setup-catalog.json` — the generated catalog the CLI embeds.
- `.claude-plugin/` — exposes the shared skills through the Claude Code marketplace.
- `drafts/` — unreviewed skills. Not published.
- `personal/` — machine-specific skills. Never published.

## Contributing

Install the JavaScript workspace and run the repository gates:

```bash
npm install
npm run check
npm run audit
```

Check the Rust CLI separately:

```bash
cargo fmt --manifest-path cli/ai-setup/Cargo.toml -- --check
cargo clippy --manifest-path cli/ai-setup/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path cli/ai-setup/Cargo.toml
```

If you change a reviewed skill or package catalog metadata, regenerate the embedded catalog:

```bash
npm run catalog:generate
```

For local Pi extension development, use `scripts/sync-pi-extensions.sh status` and `scripts/sync-pi-extensions.sh link`.

## Credits

This repo builds on other people's work:

- Several of the coding skills are adapted from [Matt Pocock's skills](https://github.com/mattpocock/skills), and the [research](skills/research/SKILL.md) skill is his, copied verbatim.
- [reviewr](plugins/herdr-reviewr/) began as a fork of [persiyanov/herdr-reviewr](https://github.com/persiyanov/herdr-reviewr) (MIT). It has diverged since — GitLab MRs, Windows builds, remote diff review — but the review pane at its heart is his design.
- [subagents](plugins/pi-subagents/) is a vendored fork of [nicobailon/pi-subagents](https://github.com/nicobailon/pi-subagents) (MIT); [web-access](plugins/web-access/) and [rewind](plugins/rewind/) are reviewed distributions of his [pi-web-access](https://github.com/nicobailon/pi-web-access) and [pi-rewind-hook](https://github.com/nicobailon/pi-rewind-hook).
- [mindwalk](plugins/mindwalk/) is vendored from [cosmtrek/mindwalk](https://github.com/cosmtrek/mindwalk) (MIT), created by Ricko Yu. Pi session support began with [ZacharyQin's upstream PR](https://github.com/cosmtrek/mindwalk/pull/4).
- [ask-user](plugins/ask-user/) and [claude-bridge](plugins/claude-bridge/) are reviewed distributions of [edlsh/pi-ask-user](https://github.com/edlsh/pi-ask-user) and [elidickinson/pi-claude-bridge](https://github.com/elidickinson/pi-claude-bridge).

Each package's README and `THIRD_PARTY_NOTICES.md` record the exact upstream version. Thanks, all.

## License

[MIT](LICENSE)
