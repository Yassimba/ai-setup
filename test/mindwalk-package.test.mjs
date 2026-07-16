import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";

const packageRoot = new URL("../plugins/mindwalk/", import.meta.url);
const runtimePackages = [
  "@fontsource-variable/fraunces",
  "@fontsource-variable/schibsted-grotesk",
  "lucide-react",
  "react",
  "react-dom",
  "scheduler",
  "three",
  "zustand",
];

async function readJSON(path) {
  return JSON.parse(await readFile(new URL(path, packageRoot), "utf8"));
}

test("the Mindwalk package is installable by Pi and bundles its native runtime", async () => {
  const manifest = await readJSON("package.json");
  assert.equal(manifest.name, "@yassimba/pi-mindwalk");
  assert.deepEqual(manifest.pi.extensions, ["./index.ts"]);
  assert.ok(manifest.files.includes("dist"));
  assert.equal(manifest.scripts.prepack, "npm run build:binaries");

  const builder = await readFile(new URL("scripts/build-binaries.mjs", packageRoot), "utf8");
  for (const target of [
    '["darwin", "amd64"]',
    '["darwin", "arm64"]',
    '["linux", "amd64"]',
    '["linux", "arm64"]',
    '["windows", "amd64"]',
    '["windows", "arm64"]',
  ]) {
    assert.ok(builder.includes(target), `missing build target ${target}`);
  }
});

test("the embedded web runtime keeps exact third-party license notices", async () => {
  const lock = await readJSON("web/package-lock.json");
  const notices = await readFile(new URL("THIRD_PARTY_NOTICES.md", packageRoot), "utf8");

  for (const name of runtimePackages) {
    const version = lock.packages[`node_modules/${name}`]?.version;
    assert.ok(version, `${name} must remain in the web lock`);
    assert.match(notices, new RegExp(`## ${name.replaceAll("/", "\\/")}@${version}`));
  }
  assert.match(notices, /SIL OPEN FONT LICENSE Version 1\.1/);
  assert.match(notices, /ISC License/);
  assert.match(notices, /MIT License/);
  assert.match(notices, /## Go standard library 1\.25/);
  assert.match(notices, /Copyright 2009 The Go Authors/);
});
