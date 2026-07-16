import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import { dirname, join, relative } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");

async function findPowershellScripts(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const scripts = [];
  for (const entry of entries) {
    if (entry.name === "node_modules" || entry.name === "target") continue;
    const path = join(directory, entry.name);
    if (entry.isDirectory()) {
      scripts.push(...(await findPowershellScripts(path)));
    } else if (entry.name.endsWith(".ps1")) {
      scripts.push(path);
    }
  }
  return scripts;
}

// Windows PowerShell 5.1 parses BOM-less .ps1 files as ANSI, so UTF-8
// punctuation decodes to cp1252 smart quotes that can terminate strings
// mid-line and break the whole script. Keep install/build scripts ASCII.
test("PowerShell scripts contain only ASCII", async () => {
  const scripts = await findPowershellScripts(repoRoot);
  assert.ok(scripts.length > 0, "expected to find .ps1 scripts");
  for (const script of scripts) {
    const content = await readFile(script, "utf8");
    const lines = content.split("\n");
    for (const [index, line] of lines.entries()) {
      const nonAscii = [...line].find((char) => char.codePointAt(0) > 0x7f);
      assert.equal(
        nonAscii,
        undefined,
        `${relative(repoRoot, script)}:${index + 1} contains non-ASCII: ${line.trim()}`,
      );
    }
  }
});
