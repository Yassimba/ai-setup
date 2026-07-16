---
description: Python-specific refactoring smells (modernization, typing) and the exact quality-gate commands.
---

# Python Targets

Load when the refactoring target is Python. Only what's Python-specific lives here — the language-agnostic smells stay in SKILL.md, and their Python resolutions (StrEnum, frozen dataclass, protocols) are shown as full patterns in [patterns.md](patterns.md).

## Modernization

- `Optional[X]` → `X | None`
- `typing.List` / `typing.Dict` → `list` / `dict`
- Mutable default arguments → `None` + assign inside, or tuple/frozenset
- Missing `frozen=True` / `slots=True` on pure-data dataclasses
- Old-style type aliases → `type X = ...`
- Boolean positional parameters → keyword-only

## Quality gate

After each refactoring commit:

```
uv run ruff check && uv run ty check && uv run pytest
```

Full verification adds complexity and boundary checks:

```
uv run complexipy src/ -mx 10 && uv run tach check
```
