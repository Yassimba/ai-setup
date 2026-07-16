import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { copyFile, mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { createCiPlan } from "../scripts/ci-plan.mjs";
import { bumpVersion, discoverReleaseComponents, inferBump } from "../scripts/release-lib.mjs";
import { resolveReleasePlan } from "../scripts/resolve-release.mjs";

async function fixture() {
  const root = await mkdtemp(join(tmpdir(), "release-automation-"));
  await mkdir(join(root, "plugins", "pi-example"), { recursive: true });
  await mkdir(join(root, "plugins", "herdr-prebuilt"), { recursive: true });
  await mkdir(join(root, "plugins", "herdr-source"), { recursive: true });
  await mkdir(join(root, "cli", "ai-setup"), { recursive: true });
  await writeFile(
    join(root, "plugins", "pi-example", "package.json"),
    JSON.stringify({ name: "@example/pi-example", version: "1.2.3" }),
  );
  await writeFile(
    join(root, "plugins", "herdr-prebuilt", "Cargo.toml"),
    '[package]\nname = "herdr-prebuilt"\nversion = "2.0.0"\nrust-version = "1.90"\n',
  );
  await writeFile(
    join(root, "plugins", "herdr-prebuilt", "herdr-plugin.toml"),
    'version = "2.0.0"\nplatforms = ["linux", "windows"]\ncommand = ["bash", "herdr/install.sh"]\n',
  );
  await writeFile(
    join(root, "plugins", "herdr-source", "Cargo.toml"),
    '[package]\nname = "herdr-source"\nversion = "0.4.0"\n',
  );
  await writeFile(
    join(root, "plugins", "herdr-source", "herdr-plugin.toml"),
    'version = "0.4.0"\nplatforms = ["linux", "windows"]\ncommand = ["bash", "herdr/build.sh"]\n',
  );
  await writeFile(
    join(root, "cli", "ai-setup", "Cargo.toml"),
    '[package]\nname = "ai-setup"\nversion = "0.1.0"\n',
  );
  await writeFile(join(root, "install.sh"), 'VERSION="0.1.0"\n');
  await writeFile(join(root, "install.ps1"), '$Version = "0.1.0"\n');
  return root;
}

test("npm publishing treats component paths as local directories", async () => {
  const workflow = await readFile(".github/workflows/publish-pi-package.yml", "utf8");

  assert.match(workflow, /NODE_AUTH_TOKEN: \$\{\{ secrets\.NPM_TOKEN \}\}/);
  assert.match(workflow, /npm publish "\.\/\$PACKAGE_PATH"/);
});

test("manual publishing reruns target the commit that created the release plan", async () => {
  const workflow = await readFile(".github/workflows/publish-pi-package.yml", "utf8");

  assert.match(workflow, /release_sha=\$\(git log -1 --format=%H -- \.release-plan\.json\)/);
  assert.doesNotMatch(workflow, /^\s+GITHUB_SHA:/m);
  assert.equal(
    workflow.match(/RELEASE_SHA: \$\{\{ needs\.resolve\.outputs\.release_sha \}\}/g)?.length,
    3,
  );
  assert.equal(
    workflow.match(/git show "\$GITHUB_SHA:scripts\/ensure-github-release\.sh"/g)?.length,
    3,
  );
  assert.equal(workflow.match(/ref: \$\{\{ needs\.resolve\.outputs\.release_sha \}\}/g)?.length, 4);
});

test("release continuation targets the repository without requiring a checkout", async () => {
  const workflow = await readFile(".github/workflows/publish-pi-package.yml", "utf8");

  assert.match(workflow, /gh workflow run release-pr\.yml --repo "\$GITHUB_REPOSITORY" --ref main/);
});

test("discovers npm, prebuilt Rust, source Rust, and CLI releases from manifests", async () => {
  const root = await fixture();
  const components = discoverReleaseComponents(root);

  assert.deepEqual(
    components.map(({ id, distribution, version }) => ({ id, distribution, version })),
    [
      { id: "ai-setup", distribution: "rust-binary", version: "0.1.0" },
      { id: "herdr-prebuilt", distribution: "rust-binary", version: "2.0.0" },
      { id: "herdr-source", distribution: "rust-source", version: "0.4.0" },
      { id: "pi-example", distribution: "npm", version: "1.2.3" },
    ],
  );
});

test("rejects drift between CLI and installer versions", async () => {
  const root = await fixture();
  await writeFile(join(root, "install.sh"), 'VERSION="9.9.9"\n');

  assert.throws(() => discoverReleaseComponents(root), /Cargo 0\.1\.0, install\.sh 9\.9\.9/);
});

test("infers the highest semantic bump from conventional commits", () => {
  const files = ["plugins/example/src/index.ts"];
  assert.equal(inferBump([{ subject: "fix: repair it", body: "", files }]), "patch");
  assert.equal(
    inferBump([
      { subject: "fix: repair it", body: "", files },
      { subject: "feat(example): add it", body: "", files },
    ]),
    "minor",
  );
  assert.equal(inferBump([{ subject: "refactor!: replace API", body: "", files }]), "major");
  assert.equal(
    inferBump([{ subject: "docs: explain it", body: "", files: ["plugins/example/README.md"] }]),
    undefined,
  );
  assert.equal(bumpVersion("1.2.3", "major"), "2.0.0");
});

test("CI matrices include only affected Rust projects and Windows-capable projects", async () => {
  const root = await fixture();
  const plan = createCiPlan(root, ["plugins/herdr-source/src/main.rs"]);

  assert.deepEqual(
    plan.linux.map(({ id }) => id),
    ["herdr-source"],
  );
  assert.deepEqual(
    plan.windows.map(({ id }) => id),
    ["herdr-source"],
  );
});

test("the release planner updates manifests, locks, and repeat releases", async () => {
  const root = await mkdtemp(join(tmpdir(), "release-planner-"));
  await mkdir(join(root, "scripts"), { recursive: true });
  await mkdir(join(root, "plugins", "example", "src"), { recursive: true });
  await mkdir(join(root, "plugins", "herdr-source", "src"), { recursive: true });
  await copyFile("scripts/release-lib.mjs", join(root, "scripts", "release-lib.mjs"));
  await copyFile("scripts/plan-releases.mjs", join(root, "scripts", "plan-releases.mjs"));
  await writeFile(
    join(root, "package.json"),
    JSON.stringify({
      name: "fixture",
      private: true,
      type: "module",
      scripts: { "catalog:generate": 'node -e ""' },
    }),
  );
  await writeFile(
    join(root, "package-lock.json"),
    JSON.stringify({
      lockfileVersion: 3,
      packages: { "plugins/example": { name: "@example/plugin", version: "1.2.3" } },
    }),
  );
  await writeFile(
    join(root, "plugins", "example", "package.json"),
    JSON.stringify({ name: "@example/plugin", version: "1.2.3" }),
  );
  await writeFile(join(root, "plugins", "example", "src", "index.js"), "export const value = 1;\n");
  await writeFile(
    join(root, "plugins", "herdr-source", "Cargo.toml"),
    '[package]\nname = "herdr-source"\nversion = "0.4.0"\n',
  );
  await writeFile(
    join(root, "plugins", "herdr-source", "Cargo.lock"),
    'version = 4\n\n[[package]]\nname = "herdr-source"\nversion = "0.4.0"\n',
  );
  await writeFile(
    join(root, "plugins", "herdr-source", "herdr-plugin.toml"),
    'version = "0.4.0"\nplatforms = ["linux"]\ncommand = ["bash", "herdr/build.sh"]\n',
  );
  await writeFile(join(root, "plugins", "herdr-source", "src", "main.rs"), "fn main() {}\n");
  const git = (...args) => execFileSync("git", args, { cwd: root, stdio: "ignore" });
  git("init", "-q");
  git("config", "user.email", "ci@example.test");
  git("config", "user.name", "CI");
  git("add", ".");
  git("commit", "-qm", "feat(example): initial plugin");

  execFileSync("node", ["scripts/plan-releases.mjs"], { cwd: root, stdio: "ignore" });
  let plan = JSON.parse(await readFile(join(root, ".release-plan.json"), "utf8"));
  assert.equal(plan.releases[0].bump, "initial");
  assert.equal(plan.releases[0].version, "1.2.3");
  assert.equal(plan.releases.find(({ id }) => id === "herdr-source")?.version, "0.4.0");

  git("add", ".");
  git("commit", "-qm", "chore(release): initial release");
  git("tag", "example-v1.2.3");
  git("tag", "herdr-source-v0.4.0");
  await writeFile(join(root, "plugins", "example", "src", "index.js"), "export const value = 2;\n");
  git("add", ".");
  git("commit", "-qm", "fix(example): correct value");

  execFileSync("node", ["scripts/plan-releases.mjs"], { cwd: root, stdio: "ignore" });
  plan = JSON.parse(await readFile(join(root, ".release-plan.json"), "utf8"));
  const manifest = JSON.parse(
    await readFile(join(root, "plugins", "example", "package.json"), "utf8"),
  );
  const lock = JSON.parse(await readFile(join(root, "package-lock.json"), "utf8"));
  assert.equal(plan.releases[0].bump, "patch");
  assert.equal(manifest.version, "1.2.4");
  assert.equal(lock.packages["plugins/example"].version, "1.2.4");
});

test("release plans are validated and expanded to all binary targets", async () => {
  const root = await fixture();
  const result = resolveReleasePlan(root, {
    schema: 1,
    releases: [
      {
        id: "herdr-prebuilt",
        version: "2.0.0",
        tag: "herdr-prebuilt-v2.0.0",
        distribution: "rust-binary",
      },
    ],
  });

  assert.equal(result.binaryComponents.length, 1);
  assert.equal(result.binaries.length, 6);
  assert.ok(result.binaries.some(({ target }) => target === "aarch64-pc-windows-msvc"));
});
