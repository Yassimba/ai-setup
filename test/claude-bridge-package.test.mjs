import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { readFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

import { buildSetupCatalog } from "../scripts/catalog-lib.mjs";

const executeFile = promisify(execFile);
const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const packageRoot = join(repoRoot, "plugins", "claude-bridge");

async function readManifest() {
  return JSON.parse(await readFile(join(packageRoot, "package.json"), "utf8"));
}

test("the reviewed Claude Bridge wrapper is the setup install target", async () => {
  const manifest = await readManifest();
  const catalog = await buildSetupCatalog(repoRoot);
  const claudeBridge = catalog.find((resource) => resource.label === "Claude Bridge");

  assert.equal(manifest.name, "@yassimba/pi-claude-bridge");
  assert.deepEqual(claudeBridge, {
    id: "pi-package:@yassimba/pi-claude-bridge",
    kind: "pi-package",
    group: "Pi packages",
    label: "Claude Bridge",
    description: "Use Claude Code as a Pi model provider or delegate work through AskClaude",
    installTarget: "@yassimba/pi-claude-bridge",
    nextAction: "Start Pi and use the installed package.",
  });
  assert.equal(
    catalog.some((resource) => resource.installTarget === "pi-claude-bridge"),
    false,
  );
});

test("the wrapper pins and attributes the complete upstream Pi package", async () => {
  const manifest = await readManifest();
  const readPackageFile = (name) => readFile(join(packageRoot, name), "utf8");

  assert.deepEqual(manifest.dependencies, { "pi-claude-bridge": "0.6.2" });
  assert.deepEqual(manifest.bundledDependencies, ["pi-claude-bridge"]);
  assert.deepEqual(manifest.pi, {
    extensions: ["./index.ts"],
  });
  assert.deepEqual(manifest.files, ["index.ts", "README.md", "LICENSE", "THIRD_PARTY_NOTICES.md"]);
  assert.deepEqual(
    Object.fromEntries(
      Object.entries(manifest.peerDependencies).map(([name]) => [
        name,
        manifest.peerDependenciesMeta[name]?.optional,
      ]),
    ),
    {
      "@earendil-works/pi-ai": true,
      "@earendil-works/pi-coding-agent": true,
      "@earendil-works/pi-tui": true,
      typebox: true,
    },
  );

  const [readme, license, notices] = await Promise.all([
    readPackageFile("README.md"),
    readPackageFile("LICENSE"),
    readPackageFile("THIRD_PARTY_NOTICES.md"),
  ]);
  assert.match(readme, /pi install npm:@yassimba\/pi-claude-bridge/);
  assert.match(license, /MIT License/);
  assert.match(notices, /pi-claude-bridge@0\.6\.2/);
  assert.match(notices, /Copyright \(c\) 2026 Eli Dickinson/);
  assert.match(notices, /change-case@5\.4\.4/);
  assert.match(notices, /Copyright \(c\) 2014 Blake Embrey/);
  assert.match(notices, /7e412185a62c2cdbbaee020de9e01e94e11d8851/);
  assert.match(
    notices,
    /sha512-\+MGz9zSG4np4b\/BnJOUatbXidSejQZXkj4vaIKIJzCfAEXLzFkarOxOdtKdnpNpk1aQUGCn1mXqUGkOtCFnOpg==/,
  );
});

test("the published wrapper contains the reviewed extension", async () => {
  const { stdout } = await executeFile("npm", ["pack", "--dry-run", "--json"], {
    cwd: packageRoot,
    maxBuffer: 5 * 1024 * 1024,
  });
  const [pack] = JSON.parse(stdout);
  const files = pack.files.map((file) => file.path);

  assert.deepEqual(pack.bundled, ["pi-claude-bridge"]);
  assert.equal(files.includes("index.ts"), true);
  assert.equal(files.includes("node_modules/pi-claude-bridge/package.json"), true);
  assert.equal(files.includes("node_modules/pi-claude-bridge/src/index.ts"), true);
  assert.equal(files.includes("node_modules/pi-claude-bridge/LICENSE"), true);
  assert.equal(
    files.some((path) => path.endsWith("/change-case/package.json")),
    true,
  );
  assert.equal(files.includes("README.md"), true);
  assert.equal(files.includes("THIRD_PARTY_NOTICES.md"), true);
});
