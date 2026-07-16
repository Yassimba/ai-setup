# Phase 1 Research Sub-agent Prompts

All three sub-agents use `subagent_type: "Explore"` or `general-purpose`. Put the pre-flight inventory (which surfaces exist, which package manager, which datastore) at the top of each prompt as context, and only ask about interfaces that are actually there.

## Sub-agent 1 — Application structure & user journeys

> Research this codebase. From the pre-flight inventory, the user-facing surfaces are: <CLI / API / web UI / etc.>. The package manager / runner is <uv / poetry / npm / bun / cargo / go / ...>. For each surface that exists, return:
>
> **For each CLI:**
> 1. Exact install + run commands using THIS project's tooling
> 2. Required env vars / config files
> 3. Every command and subcommand, flags, required vs optional args, what each does
> 4. Every user journey end to end, with steps and expected outputs
> 5. Interactive prompts and how to feed them non-interactively (flags, piping, heredocs)
>
> **For each API / server:**
> 1. Exact startup command using THIS project's tooling (uvicorn / gunicorn / node / bun / cargo run / go run / docker compose up …)
> 2. Base URL, auth model
> 3. Every route — method, path, request/response schema
> 4. User journeys end to end (e.g. register → authenticate → create resource → fetch it)
> 5. Error-case journeys (401, 403, 404, 422, 500 triggers)
>
> **For each web UI:**
> 1. How to start it (dev server command)
> 2. Auth flow
> 3. Every route / page
> 4. Every journey with clicks and form fills
> 5. Key interactive components (forms, modals, dropdowns, pickers)
>
> **Cross-interface journeys:** wherever data created via one surface should appear in another.
>
> Be exhaustive. Testing only covers what you identify.

## Sub-agent 2 — Datastore schema & data flows

> Research the persistence layer. Read `.env.example` (NOT `.env`), config files, and any `docker-compose.yml`. The datastore(s) detected in pre-flight: <Postgres / MySQL / SQLite / Mongo / Redis / S3 / filesystem / queue / ...>. Return:
>
> 1. Each datastore's type, connection env var(s), and the client tool to query it (`psql`, `mysql`, `sqlite3`, `mongosh`, `redis-cli`, `aws s3 ls`, `ls`, `kafkacat`, etc.)
> 2. Full schema where applicable — tables / collections / key shapes / object prefixes / topics — fields, types, relationships
> 3. For each user-facing action across ALL surfaces, what should be created / updated / deleted, and where
> 4. **Exact validation queries / commands** to verify state after each action (one per action — these get pasted into Phase 4)
> 5. Cross-interface expectations — data written via one surface should be visible / editable via another

## Sub-agent 3 — Bug hunt

> Analyze the codebase for potential bugs across ALL surfaces that exist here:
>
> 1. Logic errors — off-by-one, missing null checks, race conditions
> 2. CLI issues — missing error messages, wrong exit codes, broken piping, inconsistent output, command injection
> 3. API issues — missing auth checks, leaky error responses, missing input validation, N+1 queries
> 4. Web UI issues (if applicable) — missing error states, broken responsives, XSS
> 5. Data integrity risks — missing validation, orphaned records, wrong cascades, incorrect TTLs
> 6. Security concerns — injection (SQL / NoSQL / shell), exposed secrets in output / logs
> 7. Cross-interface inconsistencies
>
> Return a prioritized list with file:line and, for each finding, how to trigger it from a user-facing surface (the command, request, or clicks that would exercise the suspect path).
