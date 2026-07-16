# Type System — what to reach for beyond the defaults

Run type checks with `ty` (see the dedicated `ty` skill for setup and error-fixing): `uv run ty check`.

## PEP 695 syntax — always, never old-style `TypeVar`/`Generic`

```python
# Generic function — no TypeVar declaration
def first_element[T](items: Sequence[T]) -> T | None:
    return items[0] if items else None

# Generic class — no Generic[K, V] base
class Cache[K, V]:
    def __init__(self) -> None:
        self.data: dict[K, V] = {}

# Bounded
def add_numbers[T: (int, float)](a: T, b: T) -> T: ...

# Type aliases with `type` — not `X = ...` assignments
type JsonDict = dict[str, Any]
type Result[T] = Success[T] | Error

# Decorators preserving signatures — [**P, R], no ParamSpec declaration
def logged[**P, R](func: Callable[P, R]) -> Callable[P, R]:
    def wrapper(*args: P.args, **kwargs: P.kwargs) -> R:
        return func(*args, **kwargs)
    return wrapper
```

## Exhaustiveness — make ty catch the missing branch

```python
from typing import assert_never

def handle_mode(mode: Literal["read", "write"]) -> str:
    match mode:
        case "read":
            return "Reading"
        case "write":
            return "Writing"
        case _:
            assert_never(mode)  # ty errors when a new mode is added
```

## Opinions

- **Protocol over ABC inheritance** — structural typing keeps callers decoupled; reserve ABCs for genuinely shared implementation.
- **Result types over exception control-flow** for expected failures:
  ```python
  @dataclass
  class Success[T]:
      value: T

  @dataclass
  class Error:
      message: str

  type Result[T] = Success[T] | Error
  ```
- `collections.abc` in signatures (`Sequence`, `Mapping`) — accept the broadest thing you can handle, return the concrete thing you built.
