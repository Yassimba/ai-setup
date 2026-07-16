# Other Diagram Types

## State Diagram

```mermaid
stateDiagram-v2
  [*] --> Pending
  Pending --> Processing : payment_received
  Processing --> Shipped : packed
  Shipped --> Delivered : received
  Processing --> Cancelled : cancel
  Pending --> Cancelled : cancel
  Delivered --> [*]
  Cancelled --> [*]
```

### Composite States

```mermaid
stateDiagram-v2
  [*] --> Active

  state Active {
    [*] --> Idle
    Idle --> Running : start
    Running --> Idle : stop
  }

  Active --> Terminated : shutdown
  Terminated --> [*]
```

### Choice

```mermaid
stateDiagram-v2
  state check <<choice>>
  Submitted --> check
  check --> Approved : valid
  check --> Rejected : invalid
```

---

## Git Graph

```mermaid
gitGraph
  commit id: "Initial commit"
  branch develop
  checkout develop
  commit id: "Add feature A"
  commit id: "Add feature B"
  checkout main
  merge develop tag: "v1.0"
  branch hotfix
  checkout hotfix
  commit id: "Fix critical bug"
  checkout main
  merge hotfix id: "Hotfix v1.0.1"
```

---

## Gantt Chart

```mermaid
gantt
  title Project Timeline
  dateFormat YYYY-MM-DD

  section Planning
  Requirements     :a1, 2024-01-01, 7d
  Design           :a2, after a1, 5d

  section Development
  Backend API      :b1, after a2, 14d
  Frontend UI      :b2, after a2, 14d

  section Testing
  Integration Test :c1, after b1, 7d
```

---

## Pie Chart

```mermaid
pie title Language Distribution
  "JavaScript" : 45
  "Python" : 30
  "Go" : 15
  "Other" : 10
```

---

## Mind Map

```mermaid
mindmap
  root((Project))
    Frontend
      React
      CSS
      TypeScript
    Backend
      Node.js
      PostgreSQL
      Redis
    DevOps
      Docker
      Kubernetes
      CI/CD
```

---

## Timeline

```mermaid
timeline
  title Product history
  2023 : MVP launch
  2024 : Mobile app : SSO
  2025 : v2 rewrite
```

---

## User Journey

Scores are 1–5 satisfaction, per actor:

```mermaid
journey
  title Checkout experience
  section Browse
    Find product: 4: Customer
    Compare prices: 3: Customer
  section Buy
    Enter payment: 2: Customer
    Confirm order: 5: Customer, Support
```

---

## Quadrant Chart

Quadrants number counter-clockwise from top-right; points are `[x, y]` in 0–1:

```mermaid
quadrantChart
  title Effort vs Impact
  x-axis Low Effort --> High Effort
  y-axis Low Impact --> High Impact
  quadrant-1 Strategic bets
  quadrant-2 Quick wins
  quadrant-3 Skip
  quadrant-4 Money pits
  Dark mode: [0.2, 0.7]
  Realtime sync: [0.8, 0.9]
  New logo: [0.7, 0.2]
```

---

## XY Chart

```mermaid
xychart-beta
  title "Monthly signups"
  x-axis [Jan, Feb, Mar, Apr]
  y-axis "Signups" 0 --> 400
  bar [120, 180, 260, 390]
  line [120, 180, 260, 390]
```

---

## C4 Context Diagram

```mermaid
C4Context
  title System Context Diagram

  Person(user, "User", "A user of the system")
  System(system, "Main System", "The core application")
  System_Ext(external, "External API", "Third-party service")

  Rel(user, system, "Uses")
  Rel(system, external, "Calls")
```
