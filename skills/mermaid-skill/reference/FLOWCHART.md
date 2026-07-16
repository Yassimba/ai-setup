# Flowchart Syntax

## Basic Structure

```mermaid
flowchart TD
  A[Client] --> B[API Gateway]
  B --> C[Auth Service]
  B --> D[Order Service]
  D --> E[(Order DB)]
  C --> F[(User DB)]

  subgraph Services
    C
    D
  end
```

## Direction

| Keyword | Direction |
|---------|-----------|
| `TD` / `TB` | Top to bottom |
| `LR` | Left to right |
| `RL` | Right to left |
| `BT` | Bottom to top |

## Node Shapes

| Syntax | Shape | Use for |
|--------|-------|---------|
| `[text]` | Rectangle | Default nodes |
| `(text)` | Rounded rectangle | Processes |
| `{text}` | Diamond | Decisions |
| `[(text)]` | Cylinder | Databases |
| `[[text]]` | Subroutine | External calls |
| `((text))` | Circle | Start/end points |
| `>text]` | Flag | Async events |
| `{{text}}` | Hexagon | Preparation steps |

## Arrow Types

| Syntax | Style | Use for |
|--------|-------|---------|
| `-->` | Arrow | Normal flow |
| `---` | Line | Connection (no direction) |
| `-.->` | Dashed arrow | Optional/async |
| `==>` | Thick arrow | Important flow |
| `--x` | X end | Termination |
| `--o` | Circle end | Reference |

## Labels on Arrows

```mermaid
flowchart LR
  A -->|yes| B
  A -->|no| C
  B -->|"with quotes"| D
```

## Subgraphs

```mermaid
flowchart TD
  subgraph "Frontend Layer"
    A[Web App]
    B[Mobile App]
  end

  subgraph "Backend Layer"
    C[API Server]
    D[Worker]
  end

  A & B --> C
  C --> D
```

## Direction Inside a Subgraph

```mermaid
flowchart TD
  subgraph Pipeline
    direction LR
    A[Extract] --> B[Transform] --> C[Load]
  end
```

## Styling

```mermaid
flowchart LR
  A[API]:::hot --> B[(DB)]
  C[Cache] --> B
  class C cold
  classDef hot fill:#fee2e2,stroke:#b91c1c,color:#7f1d1d
  classDef cold fill:#dbeafe,stroke:#1d4ed8,color:#1e3a8a
  linkStyle 0 stroke:#b91c1c,stroke-width:3px
```

- `classDef name fill:…,stroke:…,color:…` defines a class; attach with `:::name` inline or `class A,B name`.
- `style A fill:#bbf` styles a single node without a class.
- `linkStyle N …` styles the Nth edge, counted in declaration order.

## Special Characters

Wrap in quotes for special characters:
```mermaid
flowchart LR
  A["Node: with colon"]
  B["Node (with parens)"]
  A --> B
```

## Comments

`%%` starts a comment line (must be its own line).
