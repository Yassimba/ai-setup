# API Walkthrough Recipes

Hit the running server first-hand with `curl` or `httpx`; capture the full request and response as evidence.

## Per-step capture

```bash
mkdir -p ai-docs/e2e-output/<journey>/
curl -sS -X POST http://localhost:<port>/<route> \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '<json body>' \
  -o ai-docs/e2e-output/<journey>/<step>-body.json \
  -w "HTTP %{http_code}\n" \
  > ai-docs/e2e-output/<journey>/<step>-status.txt 2>&1
```

For complex bodies or auth flows, an inline scripting helper is fine — `python -c …` with `httpx`, `node -e …` with `fetch`, etc. Whatever the project's tooling already provides. Just make sure the request body and the response are written to disk.

## What to check on each step

1. Status code matches expected (200/201/204/4xx/5xx)
2. Response body schema matches what the route claims
3. Error responses have useful messages, not stack traces or DB internals
4. DB side effects — see `db-validation.md` (and run it before declaring the step green)
5. Headers if relevant — `Content-Type`, `Location`, auth cookies

## Auth & validation sweep

Have a dedicated journey for these:

1. Missing auth → 401
2. Invalid auth (bad token, expired, wrong shape) → 401
3. Insufficient scope / wrong role → 403
4. Missing required body fields → 422 with a clear field-level message
5. Wrong content-type → 415 or 422 depending on framework
6. Body too large → 413 if a limit exists
7. Rate-limit behaviour if implemented — exceed and confirm
