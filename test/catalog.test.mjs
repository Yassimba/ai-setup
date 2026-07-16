import assert from "node:assert/strict";
import { mkdir, mkdtemp, readdir, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { buildSetupCatalog } from "../scripts/catalog-lib.mjs";

async function createCatalogFixture() {
  const repoRoot = await mkdtemp(join(tmpdir(), "pi-catalog-test-"));
  await mkdir(join(repoRoot, "skills", "reviewed"), {
    recursive: true,
  });
  await mkdir(join(repoRoot, "skills", "helper"), {
    recursive: true,
  });
  await mkdir(join(repoRoot, "personal", "private"), {
    recursive: true,
  });
  await mkdir(join(repoRoot, "plugins", "sample"), { recursive: true });
  await mkdir(join(repoRoot, "plugins", "herdr-sample"), { recursive: true });
  await writeFile(
    join(repoRoot, "skills.sh.json"),
    JSON.stringify({ groupings: [{ title: "Coding", skills: ["reviewed", "helper"] }] }),
  );
  await writeFile(
    join(repoRoot, "skills", "reviewed", "SKILL.md"),
    "---\nname: reviewed\ndescription: A reviewed skill\n---\n# Reviewed\n",
  );
  await writeFile(join(repoRoot, "skills", "reviewed", "deps.yml"), "skills:\n  - helper\n");
  await writeFile(
    join(repoRoot, "skills", "helper", "SKILL.md"),
    "---\nname: helper\ndescription: A helper skill\n---\n# Helper\n",
  );
  await writeFile(
    join(repoRoot, "personal", "private", "SKILL.md"),
    "---\nname: private\ndescription: Private\n---\n# Private\n",
  );
  await writeFile(
    join(repoRoot, "plugins", "sample", "package.json"),
    JSON.stringify({
      name: "@example/pi-sample",
      description: "Package fallback",
      aiSetup: {
        catalog: {
          label: "Sample",
          description: "Sample Pi package",
          nextAction: "Run Pi",
        },
      },
    }),
  );
  await writeFile(
    join(repoRoot, "plugins", "herdr-sample", "herdr-plugin.toml"),
    'id = "example.sample"\nname = "Sample Herdr plugin"\nversion = "1.0.0"\ndescription = "Sample Herdr capability"\n',
  );
  return repoRoot;
}

test("the setup catalog combines opted-in extensions with reviewed skills", async () => {
  const repoRoot = await createCatalogFixture();

  const catalog = await buildSetupCatalog(repoRoot);

  assert.deepEqual(catalog, [
    {
      id: "pi-package:@example/pi-sample",
      kind: "pi-package",
      group: "Pi packages",
      label: "Sample",
      description: "Sample Pi package",
      installTarget: "@example/pi-sample",
      nextAction: "Run Pi",
    },
    {
      id: "skill:reviewed",
      kind: "skill",
      group: "Coding",
      label: "reviewed",
      description: "A reviewed skill",
      installTarget: "reviewed",
      nextAction: "Ask your coding agent to use the reviewed skill.",
      dependencies: ["helper"],
    },
    {
      id: "skill:helper",
      kind: "skill",
      group: "Coding",
      label: "helper",
      description: "A helper skill",
      installTarget: "helper",
      nextAction: "Ask your coding agent to use the helper skill.",
      dependencies: [],
    },
    {
      id: "herdr-plugin:example.sample",
      kind: "herdr-plugin",
      group: "Herdr plugins",
      label: "Sample Herdr plugin",
      description: "Sample Herdr capability",
      installTarget: "Yassimba/ai-setup/plugins/herdr-sample",
      nextAction: "Run `herdr plugin list` to see the installed plugin.",
    },
  ]);
});

test("every public Pi extension package is offered in the setup catalog", async () => {
  const catalog = await buildSetupCatalog(join(import.meta.dirname, ".."));
  const offeredPackages = new Set(
    catalog.filter(({ kind }) => kind === "pi-package").map(({ installTarget }) => installTarget),
  );
  const pluginsRoot = join(import.meta.dirname, "..", "plugins");
  const pluginEntries = await readdir(pluginsRoot, { withFileTypes: true });
  const missingPackages = [];

  for (const entry of pluginEntries) {
    if (!entry.isDirectory()) continue;
    const manifestPath = join(pluginsRoot, entry.name, "package.json");
    let manifest;
    try {
      manifest = JSON.parse(await readFile(manifestPath, "utf8"));
    } catch (error) {
      if (error?.code === "ENOENT") continue;
      throw error;
    }
    if (manifest.private || !manifest.pi?.extensions?.length) continue;
    if (!offeredPackages.has(manifest.name)) missingPackages.push(manifest.name);
  }

  assert.deepEqual(missingPackages, []);
});

test("personal skills cannot enter the setup catalog", async () => {
  // personal/ lives outside skills/, so listing one in skills.sh.json fails
  // the lookup — the exclusion is structural, not a category check.
  const repoRoot = await createCatalogFixture();
  await writeFile(
    join(repoRoot, "skills.sh.json"),
    JSON.stringify({ groupings: [{ title: "Personal", skills: ["private"] }] }),
  );

  await assert.rejects(buildSetupCatalog(repoRoot), /reviewed skill not found: private/);
});

test("a dependency cycle fails catalog generation", async () => {
  const repoRoot = await createCatalogFixture();
  await writeFile(join(repoRoot, "skills", "helper", "deps.yml"), "skills:\n  - reviewed\n");

  await assert.rejects(
    buildSetupCatalog(repoRoot),
    /skill dependency cycle: reviewed -> helper -> reviewed/,
  );
});

test("a skill cannot depend on an unreviewed skill", async () => {
  const repoRoot = await createCatalogFixture();
  await writeFile(join(repoRoot, "skills", "reviewed", "deps.yml"), "skills:\n  - unlisted\n");

  await assert.rejects(
    buildSetupCatalog(repoRoot),
    /skill reviewed depends on unreviewed skill: unlisted/,
  );
});

test("private packages cannot advertise themselves in setup", async () => {
  const repoRoot = await createCatalogFixture();
  const manifestPath = join(repoRoot, "plugins", "sample", "package.json");
  const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
  await writeFile(manifestPath, JSON.stringify({ ...manifest, private: true }));

  await assert.rejects(buildSetupCatalog(repoRoot), /setup Pi package is private/);
});
