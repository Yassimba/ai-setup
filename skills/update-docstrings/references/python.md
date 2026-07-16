# Python — NumPy style

**Public/internal signal**: public = in `__all__`, or no leading underscore, or otherwise the module's API. Internal = leading underscore, _or_ excluded from `__all__`, _or_ in a conventionally-internal module. If a project bans leading underscores, use `__all__` membership alone — the split is unchanged.

**Summary mood**: imperative — "Return X", not "Returns X".

## Public shape

```python
def parse_capability_id(dotted: str) -> CapabilityId:
    """Split a dotted Capability ID into its three structured segments.

    Optional one-paragraph extended description: invariants, side effects, or
    why the function exists — context the signature cannot convey.

    Parameters
    ----------
    dotted : str
        A ``<owner>.<kind>.<name>`` string. The first two dots separate the
        segments; further dots are preserved inside `name`.

    Returns
    -------
    CapabilityId
        Structured form with `owner`, `kind_segment`, and `name`.

    Raises
    ------
    ValueError
        If `dotted` has fewer than two ``.`` separators.

    Examples
    --------
    >>> parse_capability_id("turbine.lint.placeholder")
    CapabilityId(owner=Owner(value='turbine'), kind_segment='lint', name='placeholder')
    """
```

Dialect specifics on top of the core section rules:

- **Parameters**: every parameter except `self` / `cls`. Append `optional` when a default exists.
- **Returns**: omit entirely for `None`-returning procedures.
- **Examples**: doctest form — it runs under `pytest --doctest-modules`.
- Optional sections: **Yields** (generators, replaces Returns), **Notes** (maintainer-only rationale), **See Also** (tightly-coupled symbols).

## Internal helpers

One or two sentences, no sections, no type restatement:

```python
def _coerce_default(value: object) -> object:
    """Coerce list/set defaults to tuples so frozen models stay hashable.

    Called by the field-default normaliser before the value reaches Pydantic;
    tuples survive ``model_config.frozen = True``, mutable types do not.
    """
```

## Classes

```python
@dataclass(frozen=True, slots=True)
class CapabilityId:
    """The ``<owner>.<kind>.<name>`` identity of a Capability.

    Used in selectors, Diagnostic codes, ``noqa`` directives, and editor
    displays. Stable across an extension's major version.
    """
```

- **Dataclasses / Pydantic models**: document the class; skip per-attribute prose unless an attribute's meaning is non-obvious (the field type is already in the body).
- **Classes with an explicit `__init__`**: the `Parameters` section goes on the _class_ docstring, not `__init__`. Keep `__init__` itself short or undocumented.
- **`Protocol`s**: the class docstring states what implementers promise; each method docstring states what implementations must do.

## Methods and special cases

Methods follow the function rules (skip `self` from Parameters). Each still gets at least one line:

- **`__repr__` / `__eq__` / `__hash__` and other dunders**: one line on the behaviour (`"""Value-based equality across all fields."""`), even when it matches the language default.
- **`@property`**: treat as an attribute — one line (`"""The resolved absolute project root."""`).
- **`@overload`**: docstring on the implementation, not the overloads.
- **Overrides**: a method that overrides a base class without refining its contract gets `"""See base class."""` — a full docstring only when the override changes the promise.
- **Test functions**: one line on the behaviour under test (`"""Reject a duplicate Capability ID with a clear diagnostic."""`) — the name says _what_, the docstring says _what correct looks like_.
- **Inner closures / nested `def`**: one line on the role.

## Module docstring

The file's first statement: one sentence naming the role the module plays in the system — orientation first, mechanics after. Add a short paragraph on the mechanics only when the file's location doesn't already make the context obvious.

```python
"""Adapter discovering Capability Extensions through Python entry points.

Loading an entry point imports its target module and fires every
``@capability.<kind>`` decorator inside it. This adapter snapshots the
resulting Capabilities between entry points so each manifest pairs with
exactly its own contributions.
"""
```

Avoid: restating the import path, listing the module's symbols (their own docstrings cover that), or architecture essays (those belong in `architecture.md` / ADRs).

## Linter

`ruff check --select D` (pydocstyle rules); set `convention = "numpy"` under `[tool.ruff.lint.pydocstyle]` if the project hasn't. Doctests run via `pytest --doctest-modules` or `python -m doctest`.
