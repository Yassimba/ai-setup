import assert from "node:assert/strict";
import { test } from "node:test";
import { fnv1a64, localTargetFor, socketPathFor, windowsKey } from "../src/extension/client.ts";
import { renderContext } from "../src/extension/index.ts";

test("fnv1a64 matches the reference vectors reviewr's Rust side uses", () => {
  // Standard FNV-1a 64 test vectors: any drift here means the two sides derive
  // different socket paths and never meet.
  assert.equal(fnv1a64(""), "cbf29ce484222325");
  assert.equal(fnv1a64("a"), "af63dc4c8601ec8c");
  assert.equal(fnv1a64("foobar"), "85944171f73967e8");
});

test("socket paths are deterministic, per-user, and platform-shaped", () => {
  const a = socketPathFor("/work/tree");
  assert.equal(a, socketPathFor("/work/tree"));
  assert.notEqual(a, socketPathFor("/other/tree"));
  assert.match(a, /reviewr-collab-[0-9a-f]{16}/);
  if (process.platform === "win32") {
    assert.ok(a.startsWith("\\\\.\\pipe\\"));
  } else {
    assert.ok(a.endsWith(".sock"));
  }
  assert.equal(localTargetFor("/work/tree"), "local:/work/tree");
});

test("windows worktree keys drop the verbatim prefix and case", () => {
  assert.equal(windowsKey("\\\\?\\C:\\Users\\Jan Dirk\\repo"), "c:\\users\\jan dirk\\repo");
  assert.equal(windowsKey("\\\\?\\UNC\\host\\share\\repo"), "\\\\host\\share\\repo");
  assert.equal(windowsKey("C:\\Users\\Jan Dirk\\repo"), "c:\\users\\jan dirk\\repo");
  assert.equal(windowsKey("c:\\already\\lower"), "c:\\already\\lower");
});

test("windows-normalized hash vectors match reviewr's Rust side", () => {
  // Twin: herdr-reviewr/src/collab/context.rs `key_hash_vectors_match_the_pi_extension` —
  // the same inputs must hash identically there, or the two sides derive different socket
  // names and target keys and never meet.
  assert.equal(fnv1a64("alice|c:\\users\\jan dirk\\repo"), "a111569fc1f3afa1");
  assert.equal(
    fnv1a64(`alice|${windowsKey("\\\\?\\C:\\Users\\Jan Dirk\\repo")}`),
    "a111569fc1f3afa1",
  );
});

test("renderContext narrates target, location, tray evidence, and incompleteness", () => {
  const text = renderContext({
    target: "github:github.com/acme/widgets#7",
    source: "github-pr",
    worktree: "/work/tree",
    location: { path: "src/a.rs", side: "new", line: 9, start_line: 4 },
    patch: "+added line\n-removed line",
    item: {
      kind: "finding",
      author: "rev",
      anchor: "src/a.rs:9",
      body: "boundary bug",
      thread: "T1",
      replies: [{ author: "alice", body: "agreed" }],
      replies_complete: false,
    },
    tray: [{ alias: "C1", kind: "comment", author: "bob", anchor: "comment", body: "ship it" }],
  });
  assert.match(text, /reviewing: github:github\.com\/acme\/widgets#7 \(github-pr\)/);
  assert.match(text, /location: src\/a\.rs:4-9 \(new side\)/);
  assert.match(text, /discussion id: T1/);
  assert.match(text, /↳ @alice: agreed/);
  assert.match(text, /reply chain incomplete/);
  assert.match(text, /C1 comment by @bob/);
  assert.match(text, /\+added line/);
});

test("renderContext survives an empty snapshot", () => {
  const text = renderContext(null);
  assert.match(text, /\[reviewr context\]/);
});
