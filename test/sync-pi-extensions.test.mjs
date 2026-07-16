import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { lstat, mkdir, mkdtemp, readFile, readlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const executeFile = promisify(execFile);
const realRepoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const script = join(realRepoRoot, "scripts", "sync-pi-extensions.sh");

async function createFixture() {
  const repoRoot = await mkdtemp(join(tmpdir(), "pi-extension-sync-repo-"));
  const agentDir = await mkdtemp(join(tmpdir(), "pi-extension-sync-agent-"));
  const source = join(repoRoot, "plugins", "example", "src");
  await mkdir(source, { recursive: true });
  await writeFile(
    join(repoRoot, "plugins", "example", "package.json"),
    JSON.stringify({ name: "@fixture/example", pi: { extensions: ["./src/index.ts"] } }),
  );
  await writeFile(join(source, "index.ts"), "export default function example() {}\n");
  await writeFile(join(source, "helper.ts"), "export const value = 1;\n");
  await executeFile("git", ["init", "-q"], { cwd: repoRoot });
  await executeFile("git", ["add", "."], { cwd: repoRoot });
  await executeFile(
    "git",
    [
      "-c",
      "user.name=Test",
      "-c",
      "user.email=test@example.com",
      "-c",
      "commit.gpgsign=false",
      "commit",
      "-qm",
      "fixture",
    ],
    { cwd: repoRoot },
  );
  return { repoRoot, agentDir, source };
}

async function run(fixture, ...args) {
  return executeFile("bash", [script, ...args], {
    cwd: fixture.repoRoot,
    env: {
      ...process.env,
      PI_EXTENSIONS_REPO: fixture.repoRoot,
      PI_CODING_AGENT_DIR: fixture.agentDir,
    },
  });
}

test("link creates a live global symlink and never overwrites a divergent copy", async () => {
  const fixture = await createFixture();
  const target = join(fixture.agentDir, "extensions", "example");

  const initial = await run(fixture, "status");
  assert.match(initial.stdout, /- example {2}absent/);
  await run(fixture, "link");
  assert.equal((await lstat(target)).isSymbolicLink(), true);
  assert.equal(await readlink(target), fixture.source);

  await writeFile(join(target, "helper.ts"), "export const value = 2;\n");
  assert.equal(
    await readFile(join(fixture.source, "helper.ts"), "utf8"),
    "export const value = 2;\n",
  );

  await run(fixture, "unlink", "example");
  assert.equal((await lstat(target)).isSymbolicLink(), false);
  await writeFile(join(target, "index.ts"), "global divergent copy\n");
  const linked = await run(fixture, "link", "example");
  assert.match(linked.stdout, /diverged example/);
  assert.equal((await lstat(target)).isSymbolicLink(), false);
  assert.match(await readFile(join(target, "index.ts"), "utf8"), /global divergent copy/);
});

test("pull imports a clean divergent copy and legacy files are treated as conflicts", async () => {
  const fixture = await createFixture();
  const extensions = join(fixture.agentDir, "extensions");
  const target = join(extensions, "example");

  await run(fixture, "link", "example");
  await run(fixture, "unlink", "example");
  await writeFile(join(target, "index.ts"), "pulled from global\n");
  const pulled = await run(fixture, "pull", "example");
  assert.match(pulled.stdout, /pulled {3}example/);
  assert.equal(await readFile(join(fixture.source, "index.ts"), "utf8"), "pulled from global\n");

  const legacyFixture = await createFixture();
  const legacyExtensions = join(legacyFixture.agentDir, "extensions");
  await mkdir(legacyExtensions, { recursive: true });
  const legacy = join(legacyExtensions, "example.ts");
  await writeFile(legacy, "legacy divergent file\n");
  const conflict = await run(legacyFixture, "link", "example");
  assert.match(conflict.stdout, /conflict example/);
  await assert.rejects(lstat(join(legacyExtensions, "example")));
  assert.equal(await readFile(legacy, "utf8"), "legacy divergent file\n");
});

test("dependency-owned extension entrypoints stay outside source sync", async () => {
  const fixture = await createFixture();
  const wrapper = join(fixture.repoRoot, "plugins", "wrapper");
  await mkdir(wrapper, { recursive: true });
  await writeFile(
    join(wrapper, "package.json"),
    JSON.stringify({
      name: "@fixture/wrapper",
      pi: { extensions: ["./node_modules/upstream/index.ts"] },
    }),
  );

  const status = await run(fixture, "status");

  assert.match(status.stdout, /repo: 1 Pi extension entrypoints/);
  assert.doesNotMatch(status.stdout, /wrapper|upstream/);
});
