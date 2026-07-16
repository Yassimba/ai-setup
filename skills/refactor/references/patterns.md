---
description: 7 refactoring patterns with before/after Python code — load when applying a change.
---

# Refactoring Pattern Catalog

7 refactoring patterns with before/after Python code. The shapes generalize to any language; the concrete Python smells live in [python.md](python.md).

---

## 1. Extract Function

**Smell:** Function does too many things, exceeds 30 lines, or has deeply nested logic.

### Before

```python
def process_order(order: Order, db: Database) -> OrderResult:
    # validate
    if not order.items:
        raise ValueError("Empty order")
    if order.total < 0:
        raise ValueError("Negative total")
    for item in order.items:
        if item.quantity <= 0:
            raise ValueError(f"Invalid quantity for {item.name}")

    # calculate discounts
    discount = Decimal("0")
    if order.total > 100:
        discount = order.total * Decimal("0.1")
    if order.customer.is_vip:
        discount += order.total * Decimal("0.05")
    final_total = order.total - discount

    # persist
    record = db.insert_order(order.customer.id, final_total)
    for item in order.items:
        db.insert_line_item(record.id, item.name, item.quantity, item.price)
    return OrderResult(id=record.id, total=final_total, discount=discount)
```

### After

```python
def validate_order(order: Order) -> None:
    if not order.items:
        raise ValueError("Empty order")
    if order.total < 0:
        raise ValueError("Negative total")
    for item in order.items:
        if item.quantity <= 0:
            raise ValueError(f"Invalid quantity for {item.name}")

def calculate_discount(order: Order) -> Decimal:
    discount = order.total * Decimal("0.1") if order.total > 100 else Decimal("0")
    if order.customer.is_vip:
        discount += order.total * Decimal("0.05")
    return discount

def process_order(order: Order, db: Database) -> OrderResult:
    validate_order(order)
    discount = calculate_discount(order)
    final_total = order.total - discount
    record = db.insert_order(order.customer.id, final_total)
    for item in order.items:
        db.insert_line_item(record.id, item.name, item.quantity, item.price)
    return OrderResult(id=record.id, total=final_total, discount=discount)
```

**Why:** Each function has one job. Validation and discount logic are testable in isolation.

---

## 2. Guard Clauses (Reduce Nesting)

**Smell:** Deeply nested `if/else` blocks that obscure the happy path.

### Before

```python
def get_user_display(user: User | None, include_email: bool) -> str:
    if user is not None:
        if user.is_active:
            if include_email and user.email:
                return f"{user.name} <{user.email}>"
            else:
                return user.name
        else:
            return f"{user.name} (inactive)"
    else:
        return "anonymous"
```

### After

```python
def get_user_display(user: User | None, include_email: bool) -> str:
    if user is None:
        return "anonymous"
    if not user.is_active:
        return f"{user.name} (inactive)"
    if include_email and user.email:
        return f"{user.name} <{user.email}>"
    return user.name
```

**Why:** Early returns eliminate nesting. The happy path reads top-to-bottom without indentation.

---

## 3. Replace Conditional with StrEnum / Protocol

**Smell:** Raw string comparisons for type dispatch, or growing `if/elif` chains.

### Before

```python
TYPE_MAPPING: dict[str, str] = {
    "string": "str",
    "timestamp": "datetime",
    "integer": "int",
}

DATETIME_TYPES = {"datetime", "date", "time"}  # plain set of strings

def map_type(logical: str) -> str:
    if logical not in TYPE_MAPPING:
        raise ValueError(f"Unknown type: {logical}")
    return TYPE_MAPPING[logical]
```

### After

```python
class LogicalType(StrEnum):
    STRING = "string"
    TEXT = "text"
    INTEGER = "integer"
    TIMESTAMP = "timestamp"
    DATE = "date"
    TIME = "time"
    DATETIME = "datetime"

TYPE_MAP: dict[LogicalType, str] = {
    LogicalType.STRING: "str",
    LogicalType.TIMESTAMP: "datetime",
    LogicalType.INTEGER: "int",
}

DATETIME_TYPES: frozenset[LogicalType] = frozenset(
    {LogicalType.TIMESTAMP, LogicalType.DATE, LogicalType.TIME, LogicalType.DATETIME}
)

def map_type(logical: LogicalType) -> str:
    return TYPE_MAP[logical]
```

**Why:** Typos become type errors. Autocomplete works. `frozenset` signals immutability.

---

## 4. Replace Dict with Frozen Dataclass

**Smell:** `dict[str, Any]` passed around as a de-facto schema, with string-key access scattered everywhere.

### Before

```python
def load_config(path: Path) -> dict[str, Any]:
    raw = json.loads(path.read_text())
    return {
        "host": raw["host"],
        "port": int(raw.get("port", 5432)),
        "ssl": raw.get("ssl", False),
    }

def connect(config: dict[str, Any]) -> Connection:
    return Connection(host=config["host"], port=config["port"], ssl=config["ssl"])
```

### After

```python
@dataclass(frozen=True, slots=True)
class DbConfig:
    host: str
    port: int = 5432
    ssl: bool = False

    @classmethod
    def from_file(cls, path: Path) -> DbConfig:
        raw = json.loads(path.read_text())
        return cls(host=raw["host"], port=int(raw.get("port", 5432)), ssl=raw.get("ssl", False))

def connect(config: DbConfig) -> Connection:
    return Connection(host=config.host, port=config.port, ssl=config.ssl)
```

**Why:** Type checker catches misspelled fields. IDE provides autocomplete. Frozen means no accidental mutation.

---

## 5. Protocol Decomposition (God Class to Protocols)

**Smell:** A single ABC or class mixes discovery, file I/O, code generation, and orchestration.

### Before — God class ABC (120+ lines, 10+ methods)

```python
class Syncer[T](ABC):
    def __init__(self, generator: CodeGenerator[T], output_dir: Path) -> None:
        self.generator = generator
        self.output_dir = output_dir
        self.results: list[SyncResult] = []  # mutable state!

    @abstractmethod
    def discover(self) -> list[T]: ...

    def sync(self) -> list[SyncResult]:          # orchestration
        items = self.discover()
        self.results = [self.sync_item(i) for i in items]
        self.results.extend(self.cleanup())       # file I/O
        return self.results

    def sync_item(self, item: T) -> SyncResult:  # code gen + file I/O
        code = self.generator.generate(item)
        path = self.output_dir / self.generator.filename(item)
        ...

    def cleanup(self) -> list[SyncResult]: ...    # file I/O
    def delete_orphaned(self) -> ...: ...         # file I/O
    def update_init_file(self) -> None: ...       # file I/O
```

### After — 2 protocols + 1 data class + 1 function

```python
@runtime_checkable
class Source[T](Protocol):
    def discover(self) -> list[T]: ...

@runtime_checkable
class Renderer[T](Protocol):
    def render(self, item: T) -> str: ...
    def filename(self, item: T) -> str: ...

@dataclass
class FileWriter:
    output_dir: Path
    init_config: InitConfig = field(default_factory=InitConfig)

    def write(self, filename: str, code: str) -> SyncResult: ...
    def cleanup(self, synced_names: set[str]) -> list[SyncResult]: ...

def run_pipeline[T](
    source: Source[T], renderer: Renderer[T], writer: FileWriter
) -> list[SyncResult]:
    items = source.discover()
    results = [writer.write(renderer.filename(item), renderer.render(item)) for item in items]
    results.extend(writer.cleanup({r.name for r in results}))
    return results
```

**Why:** No mutable state. No inheritance. The orchestrator is a plain function composing three independent pieces. Each protocol has 1-2 methods max.

---

## 6. Unified IR (Merge Overlapping Types)

**Smell:** Multiple dataclasses/TypedDicts representing the same concept with slightly different field names.

### Before — 3 types for "a column"

```python
@dataclass(frozen=True, slots=True)
class FilterableField:
    name: str
    python_type: str
    required: bool

@dataclass(frozen=True, slots=True)
class CodegenSchema:
    table_name: str
    class_name: str
    fields: list[tuple[str, str, bool, bool]]  # (name, py_type, is_pk, is_optional)
    pk_fields: list[tuple[str, str]]
    filterable_fields: list[FilterableField] | None

@dataclass
class ParsedModel:
    class_name: str
    table_name: str
    pk_fields: list[tuple[str, str]]
    fields: dict[str, str]
```

### After — 1 unified frozen IR

```python
@dataclass(frozen=True, slots=True)
class ColumnDef:
    name: str
    python_type: str
    is_pk: bool = False
    is_optional: bool = False
    is_filterable: bool = False

@dataclass(frozen=True, slots=True)
class TableDef:
    table_name: str
    class_name: str
    columns: tuple[ColumnDef, ...]

    @property
    def pk_columns(self) -> tuple[ColumnDef, ...]:
        return tuple(c for c in self.columns if c.is_pk)

    @property
    def filterable_columns(self) -> tuple[ColumnDef, ...]:
        return tuple(c for c in self.columns if c.is_filterable)

    @property
    def has_filters(self) -> bool:
        return any(c.is_filterable for c in self.columns)
```

**Why:** `tuple` not `list` for immutability. Derived fields as `@property` instead of pre-computed. One type replaces three — all consumers share the same vocabulary.

---

## 7. Replace Inheritance with Composition

**Smell:** ABC with `abstractmethod` + concrete helper methods, forcing subclasses into a rigid hierarchy.

### Before

```python
class CodeGenerator[T](ABC):
    def generate(self, item: T) -> str:
        module = self.build_module(item)
        ast.fix_missing_locations(module)
        return ast.unparse(module)

    @abstractmethod
    def build_module(self, item: T) -> ast.Module: ...

    @abstractmethod
    def filename(self, item: T) -> str: ...
```

### After

```python
# Protocol defines the contract (structural typing)
@runtime_checkable
class Renderer[T](Protocol):
    def render(self, item: T) -> str: ...
    def filename(self, item: T) -> str: ...

# Optional convenience base — not required to satisfy the protocol
class ASTRenderer[T]:
    def render(self, item: T) -> str:
        module = self.build_module(item)
        ast.fix_missing_locations(module)
        return ast.unparse(module)

class TemplateRenderer[T]:
    def render(self, item: T) -> str:
        return self.env.get_template(self.template_name).render(self.context(item))
```

**Why:** The protocol defines the contract. `ASTRenderer` and `TemplateRenderer` are convenience bases — any class with `render()` and `filename()` satisfies the protocol without inheriting anything.
