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
const packageRoot = join(repoRoot, "plugins", "web-access");

async function readManifest() {
  return JSON.parse(await readFile(join(packageRoot, "package.json"), "utf8"));
}

test("the reviewed Web Access wrapper is the setup install target", async () => {
  const manifest = await readManifest();
  const catalog = await buildSetupCatalog(repoRoot);
  const webAccess = catalog.find((resource) => resource.label === "Web Access");

  assert.equal(manifest.name, "@yassimba/pi-web-access");
  assert.deepEqual(webAccess, {
    id: "pi-package:@yassimba/pi-web-access",
    kind: "pi-package",
    group: "Pi packages",
    label: "Web Access",
    description: "Search and fetch web, PDF, GitHub, and video content from Pi",
    installTarget: "@yassimba/pi-web-access",
    nextAction: "Start Pi and use the installed package.",
  });
  assert.equal(
    catalog.some((resource) => resource.installTarget === "pi-web-access"),
    false,
  );
});

test("the wrapper pins and attributes the complete upstream Pi package", async () => {
  const manifest = await readManifest();
  const readPackageFile = (name) => readFile(join(packageRoot, name), "utf8");

  assert.deepEqual(manifest.dependencies, { "pi-web-access": "0.13.0" });
  assert.deepEqual(manifest.bundledDependencies, ["pi-web-access"]);
  assert.deepEqual(manifest.pi, {
    extensions: ["./index.ts"],
    skills: ["./node_modules/pi-web-access/skills"],
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
  assert.match(readme, /pi install npm:@yassimba\/pi-web-access/);
  assert.match(license, /MIT License/);
  assert.match(notices, /pi-web-access@0\.13\.0/);
  assert.match(notices, /Copyright \(c\) 2025 Nico Bailon/);
  assert.match(notices, /boolbase@1\.0\.0/);
  assert.match(notices, /Copyright \(c\) 2014-2015, Felix Boehm/);
  assert.match(notices, /7bdc30a65cf77273eb9c0034647b373bda4060d7/);
  assert.match(
    notices,
    /sha512-ny0bHisMWdobmu1hcMp\/jqjaRh6pYrH7dctBK2CVyRF4ia7bP47RnOPYdG1yiks9ohtcanWir5Hl9EFap8h0zQ==/,
  );
});

test("the published wrapper contains the reviewed extension and skill", async () => {
  const { stdout } = await executeFile("npm", ["pack", "--dry-run", "--json"], {
    cwd: packageRoot,
  });
  const [pack] = JSON.parse(stdout);
  const files = pack.files.map((file) => file.path);

  assert.deepEqual(pack.bundled, ["pi-web-access"]);
  assert.equal(files.includes("index.ts"), true);
  assert.equal(files.includes("node_modules/pi-web-access/package.json"), true);
  assert.equal(files.includes("node_modules/pi-web-access/index.ts"), true);
  assert.equal(files.includes("node_modules/pi-web-access/LICENSE"), true);
  assert.equal(
    files.some((path) => path.endsWith("/boolbase/package.json")),
    true,
  );
  assert.equal(files.includes("node_modules/pi-web-access/skills/librarian/SKILL.md"), true);
  assert.equal(files.includes("README.md"), true);
  assert.equal(files.includes("THIRD_PARTY_NOTICES.md"), true);
});
