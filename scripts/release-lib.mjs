import { existsSync, readdirSync, readFileSync } from "node:fs";
import { basename, join, relative } from "node:path";

const VERSION_PATTERN = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/;

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

function tomlValue(raw, key) {
  const match = raw.match(new RegExp(`^${key}\\s*=\\s*"([^"]+)"`, "m"));
  return match?.[1];
}

function cargoPackage(raw) {
  const packageSection = raw.match(/\[package\]([\s\S]*?)(?=\n\[|$)/)?.[1] ?? "";
  return {
    name: tomlValue(packageSection, "name"),
    version: tomlValue(packageSection, "version"),
    rustVersion: tomlValue(packageSection, "rust-version"),
  };
}

function rustToolchain(componentDir, cargo) {
  const toolchainPath = join(componentDir, "rust-toolchain.toml");
  if (existsSync(toolchainPath)) {
    return tomlValue(readFileSync(toolchainPath, "utf8"), "channel") ?? "stable";
  }
  return cargo.rustVersion ?? "stable";
}

function assertVersion(version, source) {
  if (!VERSION_PATTERN.test(version ?? "")) {
    throw new Error(`invalid or missing semantic version in ${source}`);
  }
}

function npmComponent(repoRoot, id, componentDir, packagePath) {
  const manifest = readJson(packagePath);
  if (manifest.private) return undefined;
  assertVersion(manifest.version, packagePath);
  const lockPath = join(repoRoot, "package-lock.json");
  if (existsSync(lockPath)) {
    const locked = readJson(lockPath).packages?.[relative(repoRoot, componentDir)]?.version;
    if (locked !== manifest.version) {
      throw new Error(`${id}: package-lock version ${locked} does not match ${manifest.version}`);
    }
  }
  return {
    id,
    name: manifest.name,
    version: manifest.version,
    path: relative(repoRoot, componentDir),
    paths: [relative(repoRoot, componentDir)],
    manifestPath: relative(repoRoot, packagePath),
    distribution: "npm",
    tag: `${id}-v${manifest.version}`,
  };
}

function rustComponent(repoRoot, id, componentDir, cargoPath) {
  const cargo = cargoPackage(readFileSync(cargoPath, "utf8"));
  assertVersion(cargo.version, cargoPath);
  const herdrPath = join(componentDir, "herdr-plugin.toml");
  const herdrRaw = existsSync(herdrPath) ? readFileSync(herdrPath, "utf8") : "";
  const herdrVersion = herdrRaw ? tomlValue(herdrRaw, "version") : undefined;
  if (herdrVersion && herdrVersion !== cargo.version) {
    throw new Error(
      `${relative(repoRoot, herdrPath)} version ${herdrVersion} does not match Cargo ${cargo.version}`,
    );
  }
  return {
    id,
    name: cargo.name,
    bin: cargo.name,
    version: cargo.version,
    path: relative(repoRoot, componentDir),
    paths: [relative(repoRoot, componentDir)],
    manifestPath: relative(repoRoot, cargoPath),
    herdrManifestPath: existsSync(herdrPath) ? relative(repoRoot, herdrPath) : undefined,
    distribution: /herdr[\\/]install\.(?:sh|ps1)/.test(herdrRaw) ? "rust-binary" : "rust-source",
    toolchain: rustToolchain(componentDir, cargo),
    tag: `${id}-v${cargo.version}`,
  };
}

function pluginComponent(repoRoot, pluginsRoot, entry) {
  if (!entry.isDirectory()) return undefined;
  const componentDir = join(pluginsRoot, entry.name);
  const packagePath = join(componentDir, "package.json");
  if (existsSync(packagePath)) return npmComponent(repoRoot, entry.name, componentDir, packagePath);
  const cargoPath = join(componentDir, "Cargo.toml");
  if (existsSync(cargoPath)) return rustComponent(repoRoot, entry.name, componentDir, cargoPath);
  return undefined;
}

function pluginManifestPaths(repoRoot, pluginsRoot) {
  return readdirSync(pluginsRoot, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .flatMap((entry) =>
      ["package.json", "herdr-plugin.toml"]
        .map((name) => join("plugins", entry.name, name))
        .filter((path) => existsSync(join(repoRoot, path))),
    );
}

function cliComponent(repoRoot, pluginsRoot) {
  const cliDir = join(repoRoot, "cli", "ai-setup");
  const manifestPath = join(cliDir, "Cargo.toml");
  if (!existsSync(manifestPath)) return undefined;
  const cargo = cargoPackage(readFileSync(manifestPath, "utf8"));
  assertVersion(cargo.version, manifestPath);
  const shellVersion = readFileSync(join(repoRoot, "install.sh"), "utf8").match(
    /^VERSION="([^"]+)"/m,
  )?.[1];
  const powershellVersion = readFileSync(join(repoRoot, "install.ps1"), "utf8").match(
    /^\$Version = "([^"]+)"/m,
  )?.[1];
  if (shellVersion !== cargo.version || powershellVersion !== cargo.version) {
    throw new Error(
      `ai-setup: Cargo ${cargo.version}, install.sh ${shellVersion}, and install.ps1 ${powershellVersion} must match`,
    );
  }
  return {
    id: cargo.name,
    name: cargo.name,
    bin: cargo.name,
    version: cargo.version,
    path: relative(repoRoot, cliDir),
    paths: [
      "cli/ai-setup",
      "install.sh",
      "install.ps1",
      "setup-catalog.json",
      "skills",
      "skills.sh.json",
      "scripts/catalog-lib.mjs",
      "scripts/generate-setup-catalog.mjs",
      ...pluginManifestPaths(repoRoot, pluginsRoot),
    ],
    manifestPath: relative(repoRoot, manifestPath),
    distribution: "rust-binary",
    toolchain: rustToolchain(cliDir, cargo),
    tag: `${cargo.name}-v${cargo.version}`,
    versionMirrors: ["install.sh", "install.ps1"],
  };
}

export function discoverReleaseComponents(repoRoot) {
  const pluginsRoot = join(repoRoot, "plugins");
  const plugins = readdirSync(pluginsRoot, { withFileTypes: true })
    .map((entry) => pluginComponent(repoRoot, pluginsRoot, entry))
    .filter(Boolean);
  return [...plugins, cliComponent(repoRoot, pluginsRoot)]
    .filter(Boolean)
    .sort((left, right) => left.id.localeCompare(right.id));
}

export function parseSemver(version) {
  assertVersion(version, "version");
  const [core] = version.split("-");
  const [major, minor, patch] = core.split(".").map(Number);
  return { major, minor, patch };
}

export function bumpVersion(version, bump) {
  const parsed = parseSemver(version);
  if (bump === "major") return `${parsed.major + 1}.0.0`;
  if (bump === "minor") return `${parsed.major}.${parsed.minor + 1}.0`;
  if (bump === "patch") return `${parsed.major}.${parsed.minor}.${parsed.patch + 1}`;
  if (bump === "initial") return version;
  throw new Error(`unknown version bump: ${bump}`);
}

function isProductPath(path) {
  const local = path.replaceAll("\\", "/");
  const name = basename(local).toLowerCase();
  if (local.includes("/.github/") || local.includes("/test/") || local.includes("/tests/"))
    return false;
  return !["readme.md", "changelog.md", "license", "license.md", "third_party_notices.md"].includes(
    name,
  );
}

export function inferBump(commits) {
  let rank = 0;
  const bumps = [undefined, "patch", "minor", "major"];
  for (const commit of commits) {
    if (!commit.files.some(isProductPath)) continue;
    const message = `${commit.subject}\n${commit.body ?? ""}`;
    if (
      /^[a-z]+(?:\([^)]*\))?!:/i.test(commit.subject) ||
      /BREAKING[ -]CHANGE\s*:/i.test(message)
    ) {
      rank = Math.max(rank, 3);
    } else if (/^feat(?:\([^)]*\))?:/i.test(commit.subject)) {
      rank = Math.max(rank, 2);
    } else {
      rank = Math.max(rank, 1);
    }
  }
  return bumps[rank];
}

export function componentForId(components, id) {
  const component = components.find((candidate) => candidate.id === id);
  if (!component) throw new Error(`release plan references unknown component: ${id}`);
  return component;
}

export function validateReleaseEntry(component, entry) {
  if (entry.version !== component.version) {
    throw new Error(`${entry.id}: planned ${entry.version}, manifest has ${component.version}`);
  }
  if (entry.tag !== component.tag) {
    throw new Error(`${entry.id}: planned tag ${entry.tag}, expected ${component.tag}`);
  }
  if (entry.distribution !== component.distribution) {
    throw new Error(
      `${entry.id}: planned distribution changed from ${entry.distribution} to ${component.distribution}`,
    );
  }
}

export function releaseTargets() {
  return [
    { target: "aarch64-apple-darwin", os: "macos-latest" },
    { target: "x86_64-apple-darwin", os: "macos-latest" },
    { target: "x86_64-unknown-linux-gnu", os: "ubuntu-latest" },
    { target: "aarch64-unknown-linux-gnu", os: "ubuntu-latest" },
    { target: "x86_64-pc-windows-msvc", os: "windows-latest" },
    { target: "aarch64-pc-windows-msvc", os: "windows-latest" },
  ];
}
