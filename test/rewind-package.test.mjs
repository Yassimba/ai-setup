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
const packageRoot = join(repoRoot, "plugins", "rewind");

async function readManifest() {
  return JSON.parse(await readFile(join(packageRoot, "package.json"), "utf8"));
}

test("the reviewed Rewind wrapper is the setup install target", async () => {
  const manifest = await readManifest();
  const catalog = await buildSetupCatalog(repoRoot);
  const rewind = catalog.find((resource) => resource.label === "Rewind");

  assert.equal(manifest.name, "@yassimba/pi-rewind");
  assert.deepEqual(rewind, {
    id: "pi-package:@yassimba/pi-rewind",
    kind: "pi-package",
    group: "Pi packages",
    label: "Rewind",
    description:
      "File checkpoints for Pi's conversation tree: branch to an earlier message and restore files to match",
    installTarget: "@yassimba/pi-rewind",
    nextAction: "Start Pi and use the installed package.",
  });
  assert.equal(
    catalog.some((resource) => resource.installTarget === "pi-rewind-hook"),
    false,
  );
});

test("the wrapper pins and attributes the upstream Pi package", async () => {
  const manifest = await readManifest();
  const readPackageFile = (name) => readFile(join(packageRoot, name), "utf8");

  assert.deepEqual(manifest.dependencies, { "pi-rewind-hook": "1.8.4" });
  assert.deepEqual(manifest.bundledDependencies, ["pi-rewind-hook"]);
  assert.deepEqual(manifest.pi, {
    extensions: ["./index.ts"],
  });
  assert.deepEqual(manifest.files, ["index.ts", "README.md", "LICENSE", "THIRD_PARTY_NOTICES.md"]);

  const [readme, license, notices] = await Promise.all([
    readPackageFile("README.md"),
    readPackageFile("LICENSE"),
    readPackageFile("THIRD_PARTY_NOTICES.md"),
  ]);
  assert.match(readme, /pi install npm:@yassimba\/pi-rewind/);
  assert.match(license, /MIT License/);
  assert.match(notices, /pi-rewind-hook@1\.8\.4/);
  assert.match(notices, /Copyright \(c\) 2025 Nico Bailon/);
  assert.match(notices, /684f79a58fb1c30bb2a9605b573b4adf26a56381/);
  assert.match(
    notices,
    /sha512-M2V\/8pR62pdV\/el8hUULpb6abty8i\/Z6\+CidfiJ\/LB00aQ3vaMk\+8wvoz\/XaoGrW8vLhqwJrdY\/Kwaac2eOoMg==/,
  );
});

test("the published wrapper contains the reviewed extension", async () => {
  const { stdout } = await executeFile("npm", ["pack", "--dry-run", "--json"], {
    cwd: packageRoot,
  });
  const [pack] = JSON.parse(stdout);
  const files = pack.files.map((file) => file.path);

  assert.deepEqual(pack.bundled, ["pi-rewind-hook"]);
  assert.equal(files.includes("index.ts"), true);
  assert.equal(files.includes("node_modules/pi-rewind-hook/package.json"), true);
  assert.equal(files.includes("node_modules/pi-rewind-hook/index.ts"), true);
  assert.equal(files.includes("README.md"), true);
  assert.equal(files.includes("THIRD_PARTY_NOTICES.md"), true);
});
