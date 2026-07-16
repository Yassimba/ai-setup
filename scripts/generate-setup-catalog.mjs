import { readFile, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { buildSetupCatalog } from "./catalog-lib.mjs";

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(scriptDirectory, "..");
const outputPath = join(repoRoot, "setup-catalog.json");

export function renderSetupCatalog(resources) {
  const expanded = `${JSON.stringify({ schemaVersion: 1, resources }, null, 2)}\n`;
  return collapseShortStringArrays(expanded);
}

// Biome formats short arrays inline (lineWidth 100); JSON.stringify always
// expands them. Collapse string-only arrays that fit, so the generated file
// passes the repo formatter untouched.
function collapseShortStringArrays(json) {
  return json.replace(
    /^([ ]*)("[^"]+": )\[\n((?:[ ]*"(?:[^"\\]|\\.)*",?\n)+)[ ]*\](,?)$/gm,
    (match, indent, key, body, comma) => {
      const items = body
        .split("\n")
        .map((line) => line.trim().replace(/,$/, ""))
        .filter(Boolean);
      const inline = `${indent}${key}[${items.join(", ")}]${comma}`;
      return inline.length <= 100 ? inline : match;
    },
  );
}

async function generate({ check }) {
  const content = renderSetupCatalog(await buildSetupCatalog(repoRoot));
  if (check) {
    let current = "";
    try {
      current = await readFile(outputPath, "utf8");
    } catch (error) {
      if (error?.code !== "ENOENT") throw error;
    }
    if (current !== content) {
      throw new Error("setup catalog is stale; run npm run catalog:generate and commit the result");
    }
    process.stdout.write("Setup catalog is current.\n");
    return;
  }

  await writeFile(outputPath, content);
  process.stdout.write(`Generated ${outputPath}.\n`);
}

await generate({ check: process.argv.includes("--check") });
