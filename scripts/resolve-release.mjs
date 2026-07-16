import { appendFileSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  componentForId,
  discoverReleaseComponents,
  releaseTargets,
  validateReleaseEntry,
} from "./release-lib.mjs";

export function resolveReleasePlan(repoRoot, rawPlan) {
  if (rawPlan.schema !== 1 || !Array.isArray(rawPlan.releases)) {
    throw new Error("unsupported or malformed .release-plan.json");
  }
  const components = discoverReleaseComponents(repoRoot);
  const seen = new Set();
  const resolved = rawPlan.releases.map((entry) => {
    if (seen.has(entry.id)) throw new Error(`duplicate release component: ${entry.id}`);
    seen.add(entry.id);
    const component = componentForId(components, entry.id);
    validateReleaseEntry(component, entry);
    return {
      id: component.id,
      name: component.name,
      version: component.version,
      tag: component.tag,
      path: component.path,
      manifestPath: component.manifestPath,
      distribution: component.distribution,
      bin: component.bin,
      toolchain: component.toolchain,
    };
  });

  const npm = resolved.filter((entry) => entry.distribution === "npm");
  const source = resolved.filter((entry) => entry.distribution === "rust-source");
  const binaryComponents = resolved.filter((entry) => entry.distribution === "rust-binary");
  const binaries = binaryComponents.flatMap((component) =>
    releaseTargets().map((target) => ({ ...component, ...target })),
  );
  return { all: resolved, npm, source, binaryComponents, binaries };
}

function writeOutput(name, value) {
  if (!process.env.GITHUB_OUTPUT) return;
  appendFileSync(process.env.GITHUB_OUTPUT, `${name}=${JSON.stringify(value)}\n`);
}

function main() {
  const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  const planPath = process.argv[2]
    ? resolve(process.argv[2])
    : join(repoRoot, ".release-plan.json");
  const result = resolveReleasePlan(repoRoot, JSON.parse(readFileSync(planPath, "utf8")));
  for (const key of ["npm", "source", "binaryComponents", "binaries"]) {
    writeOutput(key, result[key]);
    writeOutput(`${key}Count`, result[key].length);
  }
  process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
