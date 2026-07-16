import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const pluginsRoot = join(repoRoot, "plugins");

test("single-extension Pi packages use a clean package-level label", async () => {
  const pluginNames = await readdir(pluginsRoot);
  const offenders = [];

  for (const pluginName of pluginNames) {
    const manifestPath = join(pluginsRoot, pluginName, "package.json");
    let manifest;
    try {
      manifest = JSON.parse(await readFile(manifestPath, "utf8"));
    } catch (error) {
      if (error?.code === "ENOENT") continue;
      throw error;
    }

    if (manifest.pi?.extensions?.length === 1 && manifest.pi.extensions[0] !== "./index.ts") {
      offenders.push(`${manifest.name}: ${manifest.pi.extensions[0]}`);
    }
  }

  assert.deepEqual(offenders, []);
});
