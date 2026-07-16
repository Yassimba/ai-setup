import { spawnSync } from "node:child_process";
import { chmodSync, mkdirSync, rmSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const distDir = join(packageRoot, "dist");
const targets = [
  ["darwin", "amd64"],
  ["darwin", "arm64"],
  ["linux", "amd64"],
  ["linux", "arm64"],
  ["windows", "amd64"],
  ["windows", "arm64"],
];

rmSync(distDir, { recursive: true, force: true });

for (const [goos, goarch] of targets) {
  const targetDir = join(distDir, `${goos}-${goarch}`);
  const binary = join(targetDir, goos === "windows" ? "mindwalk.exe" : "mindwalk");
  mkdirSync(targetDir, { recursive: true });
  process.stderr.write(`building mindwalk for ${goos}/${goarch}\n`);
  const result = spawnSync(
    "go",
    ["build", "-trimpath", "-ldflags=-s -w", "-o", binary, "./cmd/mindwalk"],
    {
      cwd: packageRoot,
      env: { ...process.env, CGO_ENABLED: "0", GOOS: goos, GOARCH: goarch },
      stdio: "inherit",
    },
  );
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
  if (goos !== "windows") chmodSync(binary, 0o755);
}
