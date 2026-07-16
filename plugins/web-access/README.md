# @yassimba/pi-web-access

A reviewed distribution of [Pi Web Access](https://github.com/nicobailon/pi-web-access)
for the Yassimba setup catalog. This package contains the exact upstream npm
release recorded in [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md). It adds no
runtime code of its own.

## Install

Select **Web Access** with `ai-setup add`, or install it directly:

```sh
pi install npm:@yassimba/pi-web-access
```

Restart Pi or run `/reload`. The package provides these tools:

- `web_search` searches with Exa by default and supports OpenAI, Brave, Parallel,
  Tavily, Perplexity, and Gemini.
- `fetch_content` extracts pages and PDFs, clones GitHub repositories, and handles
  YouTube or local video input.
- `get_search_content` retrieves full content saved by earlier searches.

The package also includes upstream's `librarian` skill. Search works without an API
key through Exa MCP. Add provider keys and optional video tooling as described in the
[upstream README](https://github.com/nicobailon/pi-web-access#readme).

Pi extensions run with your user account's permissions. Review the upstream source
and configuration before enabling credentials or browser-cookie access.

## Updating the bundled release

1. Inspect the new upstream npm tarball, source commit, dependencies, and licenses.
2. Set an exact `pi-web-access` version in `package.json`.
3. From `plugins/web-access`, run
   `npm install --package-lock-only --ignore-scripts --omit=peer --install-strategy=nested --workspaces=false`
   to update `npm-shrinkwrap.json`.
4. Update `THIRD_PARTY_NOTICES.md` with the version, commit, npm integrity, and any
   bundled dependency whose payload omits its license notice.
5. From the repo root, run `npm install`, `npm run catalog:generate`, and
   `npm run check`.
6. Inspect `npm pack --dry-run --json --workspace plugins/web-access` and publish a
   new wrapper version before publishing the Pi Kit catalog update.
