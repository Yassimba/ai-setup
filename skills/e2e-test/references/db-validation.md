# Datastore Validation Recipes

Query the datastore first-hand after every state-changing action; the action's evidence is the rows / keys / files / messages you read back.

## Connection

Read connection strings from the env var(s) named in `.env.example` (do NOT read `.env`). Match the client to whatever the project actually uses:

```bash
# SQL databases
psql "$DATABASE_URL" -c "SELECT ... FROM ... WHERE ..."
mysql --defaults-extra-file=<creds.cnf> -e "SELECT ..."
sqlite3 path/to/db.sqlite "SELECT ..."

# Document / KV
mongosh "$MONGO_URL" --eval 'db.<coll>.find({...}).toArray()'
redis-cli -u "$REDIS_URL" GET <key>

# Object / file storage
aws s3 ls "s3://<bucket>/<prefix>/"
ls -la <path>      # local filesystem side effects

# Queues
kafkacat -b <broker> -C -t <topic> -o end -e
```

If a scripted lookup is faster (joins, multi-step), use a throwaway in whatever language the project already uses (`python -c "import sqlalchemy …"`, `node -e "…"`, etc.). Don't pull in a new dependency just for this.

## What to verify after each action

Lift the exact validation queries from Phase-1 sub-agent 2's research. For every action, confirm the appropriate ones:

- **Created** — the new record / key / file / message exists, every field matches the input, server-defaulted fields (`id`, timestamps, status) look sane
- **Updated** — the changed fields match the request, untouched fields are preserved, any `updated_at` advanced
- **Deleted** — the record is gone (or soft-deleted), cascades fired as expected, blob removed from storage
- **Relationships** — foreign keys / references point at the right rows; join queries return the expected shape
- **No stragglers** — no orphaned children, no duplicate rows, no half-written state from a partial transaction or aborted upload

## Save the evidence

```bash
psql "$DATABASE_URL" -c "SELECT * FROM <table> WHERE id = '...'" \
    > ai-docs/e2e-output/<journey>/<step>-db-after.txt
```

(Same idea for whichever client you used.) If a step fails verification, that file IS the bug report. Keep it.

## Cross-interface check

When the same data is reachable from multiple surfaces:

1. Create via interface A
2. Run the validation query → confirm the record landed
3. Read via interface B → confirm same values
4. Mutate via B, re-query the datastore, then re-read via A. Both directions must agree.
