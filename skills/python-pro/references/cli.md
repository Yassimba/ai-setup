# CLI Development

House choice: **cyclopts** for argument parsing (not click, not typer, not argparse), **rich** for output. Both are in the model's blind spot less than you'd think — the delta here is cyclopts idiom and the operational rules.

## cyclopts

```python
import cyclopts
from enum import Enum

app = cyclopts.App()

class Environment(str, Enum):
    dev = "development"
    prod = "production"

@app.command
def deploy(
    environment: Environment,      # positional, validated by the enum
    *,                             # everything after is a --flag
    dry_run: bool = False,
    config: str | None = None,
) -> None:
    """Deploy to environment."""    # docstring becomes help text
    ...

# Nested subcommands: a sub-App registered on the parent
config_app = cyclopts.App(name="config", help="Manage configuration")
app.command(config_app)

@config_app.command
def get(key: str) -> None: ...

if __name__ == "__main__":
    app()
```

Test by invoking the app with an argv list and capturing output — do not import private helpers to bypass the parser.

## Operational rules

- **Command signatures are a public API**: keep existing names, positionals, and flags working. Evolve by adding aliases or deprecation warnings, not by renaming or removing.
- **Startup under 50ms**: lazy-import heavy dependencies inside the command that needs them (`importlib.import_module` in the handler), never at module top.
- **Exit codes**: 0 success, 1 general error, 2 misuse/invalid args, 130 SIGINT. Wrap `app()` in a `try/except KeyboardInterrupt: sys.exit(130)`.
- **Config layering**, highest priority first: CLI flags → env vars → project config → user config (`~/.config/<tool>/`) → system config → defaults.
- **Detect non-interactive**: `CI=true` or `not sys.stdout.isatty()` → no prompts (fail fast with the missing flag named), no colors (also honor `NO_COLOR`), plain output safe to pipe.
- **Error messages** follow context → problem → solution: what was attempted, what went wrong in plain language, and a concrete next command to run. Stack traces only behind `--debug`.
