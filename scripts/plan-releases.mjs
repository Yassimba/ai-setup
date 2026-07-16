import { execFileSync } from "node:child_process";
import { appendFileSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { bumpVersion, discoverReleaseComponents, inferBump } from "./release-lib.mjs";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const planPath = join(repoRoot, ".release-plan.json");

function git(args, options = {}) {
  return execFileSync("git", args, {
    cwd: repoRoot,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    ...options,
  }).trim();
}

function tagExists(tag) {
  try {
    git(["rev-parse", "--verify", `refs/tags/${tag}`]);
    return true;
  } catch {
    return false;
  }
}

function latestComponentTag(component) {
  const output = git(["tag", "--list", `${component.id}-v*`, "--sort=-version:refname"]);
  return output.split("\n").find(Boolean);
}

function commitsFor(component, baseTag) {
  const range = baseTag ? `${baseTag}..HEAD` : "HEAD";
  const hashes = git(["log", "--format=%H", range, "--", ...component.paths])
    .split("\n")
    .filter(Boolean);
  return hashes.map((hash) => {
    const subject = git(["show", "-s", "--format=%s", hash]);
    const body = git(["show", "-s", "--format=%b", hash]);
    const files = git([
      "diff-tree",
      "--no-commit-id",
      "--name-only",
      "-r",
      hash,
      "--",
      ...component.paths,
    ])
      .split("\n")
      .filter(Boolean);
    return { hash, subject, body, files };
  });
}

function versionFromTag(component, tag) {
  return tag?.startsWith(`${component.id}-v`) ? tag.slice(component.id.length + 2) : undefined;
}

function replaceCargoVersion(path, name, version) {
  const raw = readFileSync(path, "utf8");
  const packageStart = raw.indexOf("[package]");
  if (packageStart < 0) throw new Error(`${path}: missing [package]`);
  const nextSection = raw.indexOf("\n[", packageStart + 1);
  const end = nextSection < 0 ? raw.length : nextSection;
  const section = raw.slice(packageStart, end);
  const versionPattern = /(^version\s*=\s*")[^"]+("$)/m;
  if (!versionPattern.test(section)) throw new Error(`${path}: missing package version`);
  const updated = section.replace(versionPattern, `$1${version}$2`);
  writeFileSync(path, `${raw.slice(0, packageStart)}${updated}${raw.slice(end)}`);
  updateCargoLock(join(dirname(path), "Cargo.lock"), name, version);
}

function updateCargoLock(path, name, version) {
  if (!existsSync(path)) return;
  const raw = readFileSync(path, "utf8");
  const escapedName = name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const pattern = new RegExp(
    `(\\[\\[package\\]\\]\\nname = "${escapedName}"\\nversion = ")[^"]+(")`,
  );
  if (!pattern.test(raw)) throw new Error(`${path}: package ${name} not found`);
  writeFileSync(path, raw.replace(pattern, `$1${version}$2`));
}

function replaceTomlVersion(path, version) {
  const raw = readFileSync(path, "utf8");
  const versionPattern = /(^version\s*=\s*")[^"]+("$)/m;
  if (!versionPattern.test(raw)) throw new Error(`${path}: missing version`);
  writeFileSync(path, raw.replace(versionPattern, `$1${version}$2`));
}

function updatePackageLock(component, version) {
  const path = join(repoRoot, "package-lock.json");
  const lock = JSON.parse(readFileSync(path, "utf8"));
  const workspace = lock.packages?.[component.path];
  if (!workspace) throw new Error(`package-lock.json: missing workspace ${component.path}`);
  workspace.version = version;
  writeFileSync(path, `${JSON.stringify(lock, null, 2)}\n`);
}

function updateVersion(component, version) {
  const manifestPath = join(repoRoot, component.manifestPath);
  if (component.distribution === "npm") {
    const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
    manifest.version = version;
    writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
    updatePackageLock(component, version);
    return;
  }

  replaceCargoVersion(manifestPath, component.name, version);
  if (component.herdrManifestPath) {
    replaceTomlVersion(join(repoRoot, component.herdrManifestPath), version);
  }
  if (component.id === "ai-setup") {
    const shellPath = join(repoRoot, "install.sh");
    const powershellPath = join(repoRoot, "install.ps1");
    writeFileSync(
      shellPath,
      readFileSync(shellPath, "utf8").replace(/^VERSION="[^"]+"/m, `VERSION="${version}"`),
    );
    writeFileSync(
      powershellPath,
      readFileSync(powershellPath, "utf8").replace(
        /^\$Version = "[^"]+"/m,
        `$Version = "${version}"`,
      ),
    );
  }
}

function changelogEntries(commits) {
  return commits
    .filter((commit) => !/^merge\b/i.test(commit.subject))
    .map((commit) => `- ${commit.subject} (\`${commit.hash.slice(0, 7)}\`)`)
    .join("\n");
}

function updateChangelog(component, version, commits) {
  const path = join(repoRoot, component.path, "CHANGELOG.md");
  const date = new Date().toISOString().slice(0, 10);
  const entries = changelogEntries(commits) || "- Internal maintenance.";
  if (!existsSync(path)) {
    writeFileSync(
      path,
      `# Changelog\n\n## [Unreleased]\n\n## [${version}] - ${date}\n\n${entries}\n`,
    );
    return;
  }
  const raw = readFileSync(path, "utf8");
  if (raw.includes(`## [${version}]`)) return;
  const marker = "## [Unreleased]";
  if (!raw.includes(marker)) {
    writeFileSync(path, `${raw.trimEnd()}\n\n## [${version}] - ${date}\n\n${entries}\n`);
    return;
  }
  const releaseHeading = `\n\n## [${version}] - ${date}`;
  writeFileSync(path, raw.replace(marker, `${marker}${releaseHeading}`));
}

function pendingPlan() {
  if (!existsSync(planPath)) return false;
  const plan = JSON.parse(readFileSync(planPath, "utf8"));
  return plan.releases?.some((release) => !tagExists(release.tag)) ?? false;
}

export function prepareReleasePlan() {
  if (pendingPlan()) {
    return { changed: false, reason: "the current release plan has not been published yet" };
  }

  const releases = [];
  for (const component of discoverReleaseComponents(repoRoot)) {
    const baseTag = latestComponentTag(component);
    const commits = commitsFor(component, baseTag);
    if (commits.length === 0) continue;
    const bump = baseTag ? inferBump(commits) : "initial";
    if (!bump) continue;
    const taggedVersion = versionFromTag(component, baseTag);
    if (taggedVersion && taggedVersion !== component.version) {
      throw new Error(
        `${component.id}: manifest ${component.version} differs from latest tag ${taggedVersion}`,
      );
    }
    const version = bumpVersion(component.version, bump);
    updateVersion(component, version);
    updateChangelog(component, version, commits);
    releases.push({
      id: component.id,
      version,
      tag: `${component.id}-v${version}`,
      bump,
      distribution: component.distribution,
      commits: commits.map(({ hash, subject }) => ({ hash, subject })),
    });
  }

  if (releases.length === 0) return { changed: false, reason: "no releasable component changes" };
  const plan = { schema: 1, releases };
  writeFileSync(planPath, `${JSON.stringify(plan, null, 2)}\n`);
  execFileSync("npm", ["run", "catalog:generate"], { cwd: repoRoot, stdio: "inherit" });
  return { changed: true, plan };
}

function main() {
  const result = prepareReleasePlan();
  process.stdout.write(
    `${result.changed ? "prepared" : "skipped"}: ${result.changed ? `${result.plan.releases.length} release(s)` : result.reason}\n`,
  );
  if (process.env.GITHUB_OUTPUT) {
    appendFileSync(process.env.GITHUB_OUTPUT, `changed=${result.changed}\n`);
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
