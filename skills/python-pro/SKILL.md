---
name: python-pro
description: Use when writing Python 3.12+ — typed application code, async programming, CLI tools (cyclopts, rich), or pandas data manipulation.
---

# Python Pro

Type-safe, async-first, production-ready Python 3.12+: application code, CLI tools, and pandas data work.

## References

**Load the matching reference when your task touches its area** — each holds the rules and gotchas for that branch, only what the defaults get wrong.

| Reference                    | Load When                                             |
| ---------------------------- | ----------------------------------------------------- |
| `references/type-system.md`  | Generics, Protocol, type aliases, exhaustiveness      |
| `references/cli.md`          | Building or extending a CLI                           |
| `references/pandas-gotchas.md` | Any DataFrame work — cleaning, groupby, merge, perf |

Packaging and tooling have dedicated skills: use **uv** for projects/dependencies, **ty** for type checking, **ruff** for lint/format.

## Rules — General Python

- Type hints on every function signature and class attribute; `X | None` over `Optional[X]`; PEP 695 generics (`type X = ...`, `class C[T]:`, `def f[T]:`).
- Dataclasses over hand-written `__init__`; context managers for resource handling; `pathlib` over `os.path`.
- `async`/`await` for I/O-bound work; route blocking calls through `asyncio.to_thread`.
- Catch specific exception types.
- Keep default arguments immutable — a `None` sentinel for mutable defaults.
- Google-style docstrings on public APIs.

## House opinions

- **Guard clauses over nesting**: validate and return/raise early at the top of the function; the happy path reads straight down at one indent level. An `else` after a guard is a smell.
- **`TaskGroup` over bare `gather`** for structured concurrency; `asyncio.timeout()` over `wait_for`.
- **Parametrize over copy-paste tests**; property-based (hypothesis) where the invariant is crisper than examples.

## The green gate

Ship the implementation and its pytest suite together. The work is **green** when all three pass:

```bash
uv run ty check                                  # zero errors
uv run ruff check --fix && uv run ruff format
uv run pytest --cov                              # >90% coverage
```

Finish only on green.
