import assert from "node:assert/strict";
import { access, readdir, readFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const packageDirectory = join(repoRoot, "plugins", "openai-fast");

async function sourceFilesUnder(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const nestedFiles = await Promise.all(
    entries.map((entry) => {
      const path = resolve(directory, entry.name);
      if (entry.isDirectory()) return sourceFilesUnder(path);
      return entry.isFile() && entry.name.endsWith(".ts") ? [path] : [];
    }),
  );
  return nestedFiles.flat();
}

async function directPiPackageImports() {
  const imports = new Set();
  for (const sourceFile of await sourceFilesUnder(join(packageDirectory, "src"))) {
    const source = await readFile(sourceFile, "utf8");
    for (const match of source.matchAll(/from\s+["'](@earendil-works\/[^"']+)["']/g)) {
      imports.add(match[1]);
    }
  }
  return [...imports].sort();
}

test("the OpenAI Fast Mode package is installable by Pi", async () => {
  const manifest = JSON.parse(await readFile(join(packageDirectory, "package.json"), "utf8"));

  assert.equal(manifest.name, "@yassimba/pi-openai-fast");
  assert.equal(manifest.type, "module");
  assert.deepEqual(manifest.files, ["index.ts", "src", "README.md", "LICENSE"]);
  assert.deepEqual(manifest.pi.extensions, ["./index.ts"]);
  assert.deepEqual(Object.keys(manifest.peerDependencies).sort(), await directPiPackageImports());
  await access(join(packageDirectory, "index.ts"));
  await access(join(packageDirectory, "src", "index.ts"));
  await access(join(packageDirectory, "LICENSE"));
});

test("the OpenAI Fast Mode package exposes only its focused public surface", async () => {
  const extensionModule = await import(
    pathToFileURL(join(packageDirectory, "src", "index.ts")).href
  );

  assert.equal(typeof extensionModule.default, "function");
  assert.equal(typeof extensionModule.registerPiOpenAIFast, "function");
  assert.deepEqual(extensionModule.FAST_EXTENSION_CAPABILITIES, [
    "fast-mode",
    "footer-status-feedback",
  ]);
});

test("the OpenAI Fast Mode fork records its upstream provenance", async () => {
  const readme = await readFile(join(packageDirectory, "README.md"), "utf8");
  assert.match(readme, /studioarray\/pi-openai-fast/);
  assert.match(readme, /e82ed32/);
});
