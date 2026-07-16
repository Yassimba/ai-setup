# CLI Walkthrough Recipes

Drive the CLI first-hand with `Bash`; capture stdout, stderr, and exit code as evidence. Use whatever invocation the project's tooling expects (`uv run …`, `poetry run …`, `npx …`, `bun …`, `cargo run -- …`, `go run …`, or the installed binary directly).

## Per-step capture

```bash
mkdir -p ai-docs/e2e-output/<journey>/
<cli-invocation> <cmd> [args] > ai-docs/e2e-output/<journey>/<step>-stdout.txt \
                              2> ai-docs/e2e-output/<journey>/<step>-stderr.txt
echo "Exit code: $?" > ai-docs/e2e-output/<journey>/<step>-exit.txt
```

## What to check on each step

1. stdout matches expected content (read it back)
2. stderr contains no unexpected warnings
3. Exit code is 0 for success paths and non-zero for expected failure paths
4. Side effects on disk — files created / modified / removed
5. Side effects in the DB — see `db-validation.md`

## Interactive commands

Feed prompts non-interactively:

- `--yes` / `--no-input` flags if the CLI exposes them
- Pipe: `echo "answer" | <cli-invocation> <cmd>`
- Heredoc for multi-line:
  ```bash
  <cli-invocation> <cmd> <<EOF
  first answer
  second answer
  EOF
  ```

## Error-handling sweep

Have a dedicated journey for these. Each one is a manual run:

1. Missing required arg → expect helpful error + non-zero exit
2. Invalid input — wrong types, out-of-range, malformed
3. Missing dep — absent config / env var
4. Empty input — empty string, empty file, empty stdin
5. Large input (if applicable)
6. Conflicting flags — mutually exclusive options together
7. `--help` on every command and subcommand — useful, no stack traces
