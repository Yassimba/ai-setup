import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { PassThrough } from "node:stream";
import { test } from "node:test";
import { resolveMindwalkBinary } from "../plugins/mindwalk/src/binary.ts";
import { browserCommand, registerMindwalk } from "../plugins/mindwalk/src/index.ts";
import {
  MindwalkProcessManager,
  type SpawnMindwalk,
} from "../plugins/mindwalk/src/process-manager.ts";

class FakeChild extends EventEmitter {
  readonly stdout = new PassThrough();
  readonly stderr = new PassThrough();
  exitCode: number | null = null;
  killedWith: NodeJS.Signals[] = [];

  kill(signal: NodeJS.Signals = "SIGTERM"): boolean {
    this.killedWith.push(signal);
    this.exitCode = 0;
    this.emit("exit", 0, signal);
    return true;
  }
}

function fakeSpawner() {
  const calls: Array<{ command: string; args: readonly string[]; child: FakeChild }> = [];
  const spawnProcess = ((command: string, args: readonly string[]) => {
    const child = new FakeChild();
    calls.push({ command, args, child });
    queueMicrotask(() => {
      child.stdout.write("mindwalk serving http://127.0.0.1:8765/?session=pi-current-session\n");
    });
    return child;
  }) as unknown as SpawnMindwalk;
  return { calls, spawnProcess };
}

test("binary resolution selects the packaged platform artifact", () => {
  assert.equal(
    resolveMindwalkBinary({
      packageRoot: "/package",
      platform: "darwin",
      arch: "arm64",
    }),
    "/package/dist/darwin-arm64/mindwalk",
  );
  assert.equal(
    resolveMindwalkBinary({
      packageRoot: "/package",
      platform: "win32",
      arch: "x64",
    }),
    "/package/dist/windows-amd64/mindwalk.exe",
  );
  assert.throws(
    () => resolveMindwalkBinary({ packageRoot: "/package", platform: "freebsd", arch: "x64" }),
    /does not support freebsd\/x64/,
  );
});

test("process manager starts once and reopens the live session URL", async () => {
  const { calls, spawnProcess } = fakeSpawner();
  const opened: string[] = [];
  const manager = new MindwalkProcessManager({
    binaryPath: "/package/mindwalk",
    spawnProcess,
    openBrowser: async (url) => {
      opened.push(url);
      return true;
    },
  });

  const first = await manager.open("/sessions/current.jsonl", "/repo");
  const second = await manager.open("/sessions/current.jsonl", "/repo");

  assert.equal(calls.length, 1);
  assert.deepEqual(calls[0]?.args, ["open", "--no-open", "/sessions/current.jsonl"]);
  assert.deepEqual(first, {
    url: "http://127.0.0.1:8765/?session=pi-current-session",
    reused: false,
    browserOpened: true,
  });
  assert.equal(second.reused, true);
  assert.deepEqual(opened, [first.url, first.url]);

  await manager.stop();
  assert.deepEqual(calls[0]?.child.killedWith, ["SIGTERM"]);
  assert.equal(manager.currentUrl(), undefined);
});

test("process manager replaces the server when the Pi session changes", async () => {
  const { calls, spawnProcess } = fakeSpawner();
  const manager = new MindwalkProcessManager({
    binaryPath: "/package/mindwalk",
    spawnProcess,
    openBrowser: async () => true,
  });

  await manager.open("/sessions/first.jsonl", "/repo");
  await manager.open("/sessions/second.jsonl", "/repo");

  assert.equal(calls.length, 2);
  assert.deepEqual(calls[0]?.child.killedWith, ["SIGTERM"]);
  await manager.stop();
});

test("/mindwalk validates the session, reports readiness, and stops on shutdown", async () => {
  const commands = new Map<string, { handler: (args: string, ctx: never) => Promise<void> }>();
  const handlers = new Map<string, () => Promise<void>>();
  const notifications: Array<{ message: string; type: string }> = [];
  const opened: string[] = [];
  let stops = 0;
  const manager = {
    open: async (sessionFile: string) => {
      opened.push(sessionFile);
      return {
        url: "http://127.0.0.1:8765/?session=pi-demo",
        reused: false,
        browserOpened: false,
      };
    },
    stop: async () => {
      stops++;
    },
  };
  const pi = {
    registerCommand: (
      name: string,
      spec: { handler: (args: string, ctx: never) => Promise<void> },
    ) => commands.set(name, spec),
    on: (event: string, handler: () => Promise<void>) => handlers.set(event, handler),
    exec: async () => ({ code: 0 }),
  };
  registerMindwalk(pi as never, { createManager: () => manager });
  const context = (sessionFile?: string) =>
    ({
      cwd: "/repo",
      sessionManager: { getSessionFile: () => sessionFile },
      ui: {
        notify: (message: string, type: string) => notifications.push({ message, type }),
      },
    }) as never;

  await commands.get("mindwalk")?.handler("unexpected", context("/session.jsonl"));
  await commands.get("mindwalk")?.handler("", context());
  await commands.get("mindwalk")?.handler("", context("/session.jsonl"));
  await handlers.get("session_shutdown")?.();

  assert.deepEqual(opened, ["/session.jsonl"]);
  assert.equal(stops, 1);
  assert.deepEqual(notifications, [
    { message: "Usage: /mindwalk", type: "error" },
    { message: "Mindwalk needs a persisted Pi session.", type: "warning" },
    {
      message: "Mindwalk is ready at http://127.0.0.1:8765/?session=pi-demo",
      type: "warning",
    },
  ]);
});

test("browser commands avoid shell interpolation", () => {
  const url = "http://127.0.0.1:8765/?session=pi-demo";
  assert.deepEqual(browserCommand(url, "darwin"), { file: "open", args: [url] });
  assert.deepEqual(browserCommand(url, "linux"), { file: "xdg-open", args: [url] });
  assert.deepEqual(browserCommand(url, "win32"), {
    file: "rundll32",
    args: ["url.dll,FileProtocolHandler", url],
  });
});
