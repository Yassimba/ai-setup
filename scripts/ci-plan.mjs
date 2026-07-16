import { execFileSync } from "node:child_process";
import { appendFileSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { discoverReleaseComponents } from "./release-lib.mjs";

function changedFiles(repoRoot, base, head) {
  if (!base || /^0+$/.test(base)) return undefined;
  try {
    return execFileSync("git", ["diff", "--name-only", `${base}...${head}`], {
      cwd: repoRoot,
      encoding: "utf8",
    })
      .trim()
      .split("\n")
      .filter(Boolean);
  } catch {
    return undefined;
  }
}

function touchesPath(files, path) {
  return files.some((file) => file === path || file.startsWith(`${path}/`));
}

export function createCiPlan(repoRoot, files) {
  const rustComponents = discoverReleaseComponents(repoRoot).filter((component) =>
    component.distribution.startsWith("rust-"),
  );
  const sharedRustChange =
    !files ||
    files.some((file) =>
      [".github/workflows/ci.yml", "scripts/ci-plan.mjs", "scripts/release-lib.mjs"].includes(file),
    );
  const affected = rustComponents.filter(
    (component) => sharedRustChange || component.paths.some((path) => touchesPath(files, path)),
  );
  const linux = affected.map((component) => ({
    id: component.id,
    manifest: component.manifestPath,
    workspace: component.path,
    toolchain: component.toolchain,
  }));
  const windows = affected
    .filter((component) => {
      if (component.id === "ai-setup") return true;
      if (!component.herdrManifestPath) return false;
      return /platforms\s*=\s*\[[^\]]*"windows"/s.test(
        readFileSync(join(repoRoot, component.herdrManifestPath), "utf8"),
      );
    })
    .map((component) => ({
      id: component.id,
      manifest: component.manifestPath,
      workspace: component.path,
      toolchain: component.toolchain,
    }));
  return { linux, windows };
}

function output(name, value) {
  if (process.env.GITHUB_OUTPUT) {
    appendFileSync(process.env.GITHUB_OUTPUT, `${name}=${JSON.stringify(value)}\n`);
  }
}

function main() {
  const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  const files = changedFiles(repoRoot, process.env.BASE_SHA, process.env.HEAD_SHA ?? "HEAD");
  const plan = createCiPlan(repoRoot, files);
  output("linux", plan.linux);
  output("windows", plan.windows);
  output("linux_count", plan.linux.length);
  output("windows_count", plan.windows.length);
  process.stdout.write(`${JSON.stringify({ files: files ?? "all", ...plan }, null, 2)}\n`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
