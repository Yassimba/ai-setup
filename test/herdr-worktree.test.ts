import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { access, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { SessionManager } from "@earendil-works/pi-coding-agent";
import {
  buildPaneRunCommand,
  createHerdrWorktree,
  createSessionSnapshot,
  forkSessionFile,
  paneCleanupWatcherArgs,
  posixShellQuote,
  powershellQuote,
  resolveWorktrunkBin,
  tokenize,
} from "../plugins/herdr-worktree/src/index.ts";

test("creates the worktree in a sibling pane instead of a separate workspace", async () => {
  const commands: Array<{ command: string; args: string[] }> = [];
  const fakePi = {
    async exec(command: string, args: string[]) {
      commands.push({ command, args });
      if (command === "git") return { code: 1, killed: false, stdout: "", stderr: "" };
      if (command === "wt") {
        return {
          code: 0,
          killed: false,
          stdout: `${JSON.stringify({ path: "/tmp/worktree" })}\n`,
          stderr: "",
        };
      }
      if (command === "herdr" && args[0] === "pane" && args[1] === "split") {
        return {
          code: 0,
          killed: false,
          stdout: `${JSON.stringify({ result: { pane: { pane_id: "new-pane" } } })}\n`,
          stderr: "",
        };
      }
      throw new Error(`Unexpected command: ${command} ${args.join(" ")}`);
    },
  } as unknown as ExtensionAPI;

  const created = await createHerdrWorktree(
    fakePi,
    undefined,
    "/repo",
    {
      branch: "feature/pane",
      closeOldPane: true,
    },
    "old-pane",
  );

  assert.equal(created.rootPaneId, "new-pane");
  assert.deepEqual(
    commands.filter(({ command }) => command === "herdr").map(({ args }) => args),
    [["pane", "split", "old-pane", "--direction", "right"]],
  );
  assert.equal(
    commands.some(({ args }) => args.includes("worktree") && args.includes("open")),
    false,
  );
});

test("forks a fresh session before Pi has persisted its assigned session file", async () => {
  const root = await mkdtemp(join(tmpdir(), "pi-herdr-worktree-test-"));
  const sourceCwd = join(root, "source");
  const targetCwd = join(root, "target");
  await mkdir(sourceCwd);
  await mkdir(targetCwd);

  try {
    const source = SessionManager.create(sourceCwd, join(root, "source-sessions"));
    source.appendCustomEntry("unflushed-state", { preserved: true });
    const assignedFile = source.getSessionFile();
    assert.ok(assignedFile);
    await assert.rejects(access(assignedFile));

    const snapshot = await createSessionSnapshot(source);
    try {
      const forkedFile = await forkSessionFile(snapshot.file, targetCwd);
      const forked = SessionManager.open(forkedFile);

      const forkedHeader = forked.getHeader();
      assert.ok(forkedHeader);
      assert.equal(forked.getCwd(), targetCwd);
      assert.equal(forkedHeader.parentSession, undefined);
      assert.deepEqual(
        forked.getEntries().map((entry) => entry.type),
        ["custom"],
      );
    } finally {
      await snapshot.cleanup();
    }
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("quotes for POSIX shells and PowerShell", () => {
  assert.equal(posixShellQuote("it's"), `'it'"'"'s'`);
  assert.equal(powershellQuote("it's"), "'it''s'");
  assert.equal(powershellQuote("C:\\repos\\foo"), "'C:\\repos\\foo'");
});

test("builds a PowerShell pane command on Windows and a POSIX one elsewhere", () => {
  assert.equal(
    buildPaneRunCommand("C:\\sessions\\it's.jsonl", "C:\\repos\\foo", "win32"),
    "Set-Location -LiteralPath 'C:\\repos\\foo' -ErrorAction Stop; " +
      "& pi '--session' 'C:\\sessions\\it''s.jsonl' 'Moved to worktree C:\\repos\\foo. Continue.'",
  );
  assert.equal(
    buildPaneRunCommand("/tmp/session.jsonl", "/tmp/worktree", "linux"),
    "cd '/tmp/worktree' && exec " +
      "'pi' '--session' '/tmp/session.jsonl' 'Moved to worktree /tmp/worktree. Continue.'",
  );
});

test("tokenizes Windows paths without eating backslashes on win32", () => {
  assert.deepEqual(tokenize("--source C:\\repos\\foo", "win32"), ["--source", "C:\\repos\\foo"]);
  assert.deepEqual(tokenize("--source 'C:\\repos\\foo bar'", "win32"), [
    "--source",
    "C:\\repos\\foo bar",
  ]);
  assert.deepEqual(tokenize('--source "C:\\repos\\foo bar"', "win32"), [
    "--source",
    "C:\\repos\\foo bar",
  ]);
});

test("keeps backslash escape semantics on unix", () => {
  assert.deepEqual(tokenize("a\\ b", "linux"), ["a b"]);
  assert.deepEqual(tokenize('"a\\"b"', "linux"), ['a"b']);
  assert.deepEqual(tokenize("--source C:\\repos\\foo", "linux"), ["--source", "C:reposfoo"]);
});

test("resolves worktrunk on Windows, skipping the Windows Terminal alias", async () => {
  const root = await mkdtemp(join(tmpdir(), "pi-herdr-worktree-wt-"));
  try {
    const aliasDir = join(root, "Microsoft", "WindowsApps");
    const toolsDir = join(root, "tools");
    await mkdir(aliasDir, { recursive: true });
    await mkdir(toolsDir, { recursive: true });
    await writeFile(join(aliasDir, "wt.exe"), "");
    await writeFile(join(toolsDir, "wt.exe"), "");

    const pathext = ".COM;.exe";
    assert.equal(
      await resolveWorktrunkBin(
        { PATH: [aliasDir, toolsDir].join(";"), PATHEXT: pathext },
        "win32",
      ),
      join(toolsDir, "wt.exe"),
    );
    await assert.rejects(
      resolveWorktrunkBin({ PATH: aliasDir, PATHEXT: pathext }, "win32"),
      /only Windows Terminal's wt\.exe/,
    );
    await assert.rejects(
      resolveWorktrunkBin({ PATH: join(root, "empty"), PATHEXT: pathext }, "win32"),
      /not found on PATH/,
    );
    assert.equal(
      await resolveWorktrunkBin({ WORKTRUNK_BIN: "C:\\bin\\wt.exe" }, "win32"),
      "C:\\bin\\wt.exe",
    );
    assert.equal(await resolveWorktrunkBin({}, "darwin"), "wt");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("pane cleanup watcher removes the session file and closes the pane", {
  skip: process.platform === "win32",
}, async () => {
  const root = await mkdtemp(join(tmpdir(), "pi-herdr-worktree-watcher-"));
  try {
    const sessionFile = join(root, "session.jsonl");
    await writeFile(sessionFile, "{}\n");
    const binDir = join(root, "bin");
    await mkdir(binDir);
    const marker = join(root, "herdr-args.txt");
    await writeFile(join(binDir, "herdr"), `#!/bin/sh\necho "$@" > '${marker}'\n`, {
      mode: 0o755,
    });

    // A pid that cannot be alive, so the watcher's poll loop exits immediately.
    const deadPid = 0x7fffffff;
    const result = spawnSync(
      process.execPath,
      paneCleanupWatcherArgs(sessionFile, "pane-1", deadPid),
      { env: { ...process.env, PATH: `${binDir}:${process.env.PATH}` }, timeout: 30_000 },
    );

    assert.equal(result.status, 0, result.stderr?.toString());
    await assert.rejects(access(sessionFile));
    assert.equal((await readFile(marker, "utf8")).trim(), "pane close pane-1");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
