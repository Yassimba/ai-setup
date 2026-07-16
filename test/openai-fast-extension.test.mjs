import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { configPaths } from "../plugins/openai-fast/src/config.ts";
import {
  FAST_DESIRED_HANDOFF_ENV,
  FAST_REQUESTED_INACTIVE_UNSUPPORTED_MODEL_WARNING,
  FAST_STATUS_KEY,
  registerPiOpenAIFast,
} from "../plugins/openai-fast/src/index.ts";

const SUPPORTED_MODEL = { provider: "openai-codex", id: "gpt-5.5" };
const UNSUPPORTED_MODEL = { provider: "partner", id: "slow-model" };

function createPi(options = {}) {
  const handlers = new Map();
  const commands = new Map();
  return {
    api: {
      registerCommand: (name, spec) => commands.set(name, spec),
      registerFlag: () => {},
      getFlag: () => options.fastFlag === true,
      getThinkingLevel: () => options.thinkingLevel ?? "off",
      on: (event, handler) => handlers.set(event, handler),
    },
    emit: (event, payload, ctx) => handlers.get(event)({ type: event, ...payload }, ctx),
    runCommand: (name, args, ctx) => commands.get(name).handler(args, ctx),
  };
}

function createContext(options = {}) {
  const notifications = [];
  const statusCalls = [];
  const footerCalls = [];
  const ui = {};
  if (options.hasNotify !== false) {
    ui.notify = (message, type) => notifications.push({ message, type });
  }
  ui.setStatus = (key, text) => statusCalls.push({ key, text });
  if (options.captureFooter) {
    ui.setFooter = (factory) => footerCalls.push(factory);
  }
  const ctx = {
    cwd: options.cwd ?? "/work/repo",
    model: Object.hasOwn(options, "model") ? options.model : SUPPORTED_MODEL,
    ui,
    sessionManager: {
      getCwd: () => options.cwd ?? "/work/repo",
      getSessionName: () => undefined,
      getEntries: () => [],
    },
    modelRegistry: { isUsingOAuth: () => false },
    getContextUsage: () => ({ percent: 10, contextWindow: 200000 }),
  };
  return { ctx, notifications, statusCalls, footerCalls };
}

/** Register the extension against a temp HOME with optional config, run, restore env. */
async function withExtension(options, run) {
  const home = await mkdtemp(join(tmpdir(), "pi-openai-fast-ext-"));
  const previousHome = process.env.HOME;
  const previousHandoff = process.env[FAST_DESIRED_HANDOFF_ENV];
  process.env.HOME = home;
  if (options.handoff === undefined) delete process.env[FAST_DESIRED_HANDOFF_ENV];
  else process.env[FAST_DESIRED_HANDOFF_ENV] = options.handoff;
  const cwd = join(home, "repo");
  if (options.config) {
    const path = configPaths(cwd).global;
    await mkdir(dirname(path), { recursive: true });
    await writeFile(path, JSON.stringify(options.config), "utf8");
  }
  const pi = createPi(options);
  registerPiOpenAIFast(pi.api);
  const harness = createContext({ cwd, ...options.context });
  try {
    await run({ pi, cwd, home, ...harness });
  } finally {
    process.env.HOME = previousHome;
    if (previousHandoff === undefined) delete process.env[FAST_DESIRED_HANDOFF_ENV];
    else process.env[FAST_DESIRED_HANDOFF_ENV] = previousHandoff;
  }
}

async function inject(pi, ctx, payload = { model: "gpt-5.5" }) {
  return await pi.emit("before_provider_request", { payload }, ctx);
}

test("session start without any request leaves Fast Mode off", async () => {
  await withExtension({}, async ({ pi, ctx }) => {
    await pi.emit("session_start", {}, ctx);
    assert.equal(await inject(pi, ctx), undefined);
  });
});

test("/fast toggles injection on and off and writes the subagent handoff", async () => {
  await withExtension({}, async ({ pi, ctx, notifications }) => {
    await pi.emit("session_start", {}, ctx);

    await pi.runCommand("fast", "", ctx);
    assert.equal(process.env[FAST_DESIRED_HANDOFF_ENV], "1");
    const payload = { model: "gpt-5.5", input: "hello" };
    assert.deepEqual(await inject(pi, ctx, payload), {
      model: "gpt-5.5",
      input: "hello",
      service_tier: "priority",
    });
    assert.deepEqual(payload, { model: "gpt-5.5", input: "hello" }, "original payload untouched");
    assert.deepEqual(notifications, []);

    await pi.runCommand("fast", "", ctx);
    assert.equal(process.env[FAST_DESIRED_HANDOFF_ENV], "0");
    assert.equal(await inject(pi, ctx), undefined);
  });
});

test("/fast with arguments shows usage and does not toggle", async () => {
  await withExtension({}, async ({ pi, ctx, notifications }) => {
    await pi.emit("session_start", {}, ctx);
    await pi.runCommand("fast", "on", ctx);
    assert.deepEqual(notifications, [{ message: "Usage: /fast", type: "error" }]);
    assert.equal(await inject(pi, ctx), undefined);
  });
});

test("non-record payloads are never modified", async () => {
  await withExtension({}, async ({ pi, ctx }) => {
    await pi.emit("session_start", {}, ctx);
    await pi.runCommand("fast", "", ctx);
    assert.equal(await inject(pi, ctx, "raw body"), undefined);
    assert.equal(await inject(pi, ctx, [1, 2]), undefined);
    assert.equal(await inject(pi, ctx, null), undefined);
  });
});

test("requesting Fast Mode on an unsupported model warns and stays inactive", async () => {
  await withExtension(
    { context: { model: UNSUPPORTED_MODEL } },
    async ({ pi, ctx, notifications }) => {
      await pi.emit("session_start", {}, ctx);
      await pi.runCommand("fast", "", ctx);
      assert.deepEqual(notifications, [
        { message: FAST_REQUESTED_INACTIVE_UNSUPPORTED_MODEL_WARNING, type: "warning" },
      ]);
      assert.equal(await inject(pi, ctx), undefined);
    },
  );
});

test("model_select activates and deactivates Fast Mode with a single warning", async () => {
  await withExtension({ handoff: "1" }, async ({ pi, ctx, notifications }) => {
    await pi.emit("session_start", {}, ctx);
    assert.notEqual(await inject(pi, ctx), undefined);

    await pi.emit("model_select", { model: UNSUPPORTED_MODEL }, ctx);
    assert.equal(await inject(pi, ctx), undefined);
    await pi.emit("model_select", { model: { provider: "partner", id: "other" } }, ctx);
    assert.equal(
      notifications.filter((n) => n.message.includes("not supported")).length,
      1,
      "repeated unsupported selections warn only once",
    );

    await pi.emit("model_select", { model: SUPPORTED_MODEL }, ctx);
    assert.notEqual(await inject(pi, ctx), undefined);
  });
});

test("the --fast flag forces Fast Mode on and re-exports the handoff", async () => {
  await withExtension({ fastFlag: true, handoff: "0" }, async ({ pi, ctx }) => {
    await pi.emit("session_start", {}, ctx);
    assert.equal(process.env[FAST_DESIRED_HANDOFF_ENV], "1");
    assert.notEqual(await inject(pi, ctx), undefined);
  });
});

test("the handoff env variable is inherited and beats the persisted preference", async () => {
  await withExtension(
    { handoff: "0", config: { persistState: true, desiredActive: true } },
    async ({ pi, ctx }) => {
      await pi.emit("session_start", {}, ctx);
      assert.equal(await inject(pi, ctx), undefined);
    },
  );
  await withExtension({ handoff: "1" }, async ({ pi, ctx }) => {
    await pi.emit("session_start", {}, ctx);
    assert.notEqual(await inject(pi, ctx), undefined);
  });
});

test("an invalid handoff value warns and is ignored", async () => {
  await withExtension({ handoff: "yes" }, async ({ pi, ctx, notifications }) => {
    await pi.emit("session_start", {}, ctx);
    assert.equal(notifications.length, 1);
    assert.match(notifications[0].message, /Ignoring invalid PI_OPENAI_FAST_DESIRED value "yes"/);
    assert.equal(await inject(pi, ctx), undefined);
  });
});

test("persistState restores the preference across sessions and /fast persists it", async () => {
  await withExtension(
    { config: { persistState: true, desiredActive: true } },
    async ({ pi, ctx, cwd }) => {
      await pi.emit("session_start", {}, ctx);
      assert.notEqual(await inject(pi, ctx), undefined);

      await pi.runCommand("fast", "", ctx); // toggle off and persist
      const written = JSON.parse(await readFile(configPaths(cwd).global, "utf8"));
      assert.equal(written.desiredActive, false);
      assert.equal(written.persistState, true);
    },
  );
});

test("session-only /fast does not touch the config file", async () => {
  await withExtension({ config: { persistState: false } }, async ({ pi, ctx, cwd }) => {
    await pi.emit("session_start", {}, ctx);
    await pi.runCommand("fast", "", ctx);
    const written = JSON.parse(await readFile(configPaths(cwd).global, "utf8"));
    assert.equal(written.desiredActive, undefined);
  });
});

test("a failing persist write surfaces a warning", async () => {
  await withExtension(
    { config: { persistState: true } },
    async ({ pi, ctx, cwd, notifications }) => {
      await pi.emit("session_start", {}, ctx);
      await writeFile(configPaths(cwd).global, "{broken", "utf8");
      await pi.runCommand("fast", "", ctx);
      assert.equal(notifications.length, 1);
      assert.match(notifications[0].message, /needs manual repair/);
    },
  );
});

test("config load warnings are notified once and fall back to console without a notifier", async () => {
  await withExtension(
    { config: { supportedModels: "openai/gpt-5.5" } },
    async ({ pi, ctx, notifications }) => {
      await pi.emit("session_start", {}, ctx);
      await pi.emit("session_start", {}, ctx);
      assert.equal(notifications.length, 1, "second session start does not re-deliver");
      assert.match(notifications[0].message, /must be an array/);
    },
  );
  const warnings = [];
  const originalWarn = console.warn;
  console.warn = (message) => warnings.push(message);
  try {
    await withExtension(
      { config: { supportedModels: "nope" }, context: { hasNotify: false } },
      async ({ pi, ctx }) => {
        await pi.emit("session_start", {}, ctx);
      },
    );
  } finally {
    console.warn = originalWarn;
  }
  assert.equal(warnings.length, 1);
  assert.match(warnings[0], /^\[pi-openai-fast\] /);
});

test("status footer mode publishes and clears the fast status", async () => {
  await withExtension(
    { handoff: "1", config: { footer: { mode: "status" } } },
    async ({ pi, ctx, statusCalls }) => {
      await pi.emit("session_start", {}, ctx);
      assert.deepEqual(statusCalls.at(-1), { key: FAST_STATUS_KEY, text: "fast" });

      await pi.emit("model_select", { model: UNSUPPORTED_MODEL }, ctx);
      assert.deepEqual(statusCalls.at(-1), { key: FAST_STATUS_KEY, text: undefined });

      await pi.emit("model_select", { model: SUPPORTED_MODEL }, ctx);
      assert.deepEqual(statusCalls.at(-1), { key: FAST_STATUS_KEY, text: "fast" });

      await pi.emit("session_shutdown", {}, ctx);
      assert.deepEqual(statusCalls.at(-1), { key: FAST_STATUS_KEY, text: undefined });
    },
  );
});

test("replace footer mode installs the custom footer and removes it on shutdown", async () => {
  await withExtension(
    { handoff: "1", context: { captureFooter: true } },
    async ({ pi, ctx, footerCalls }) => {
      await pi.emit("session_start", {}, ctx);
      assert.equal(footerCalls.length, 1);
      assert.equal(typeof footerCalls[0], "function");

      const theme = { fg: (_c, text) => text };
      const footerData = {
        getGitBranch: () => null,
        getExtensionStatuses: () => new Map(),
        getAvailableProviderCount: () => 1,
      };
      const component = footerCalls[0]({ requestRender() {} }, theme, footerData);
      const lines = component.render(100);
      assert.equal(lines.length, 2);
      assert.match(lines[1], /gpt-5\.5 fast/);

      await pi.emit("model_select", { model: UNSUPPORTED_MODEL }, ctx);
      assert.equal(footerCalls.length, 1, "existing footer is reused, not reinstalled");
      assert.match(component.render(100)[1], /slow-model$/);

      await pi.emit("session_shutdown", {}, ctx);
      assert.equal(footerCalls.at(-1), undefined, "footer restored to the built-in one");
      assert.equal(component.isOwnedByExtension(), false);
    },
  );
});

test("off footer mode shows neither footer nor status", async () => {
  await withExtension(
    { handoff: "1", config: { footer: { mode: "off" } }, context: { captureFooter: true } },
    async ({ pi, ctx, statusCalls, footerCalls }) => {
      await pi.emit("session_start", {}, ctx);
      assert.equal(footerCalls.length, 0);
      assert.deepEqual(statusCalls.at(-1), { key: FAST_STATUS_KEY, text: undefined });
      assert.notEqual(await inject(pi, ctx), undefined, "injection still works");
    },
  );
});

test("model_select before session_start still initializes from the handoff", async () => {
  await withExtension({ handoff: "1" }, async ({ pi, ctx }) => {
    await pi.emit("model_select", { model: SUPPORTED_MODEL }, ctx);
    assert.notEqual(await inject(pi, ctx), undefined);
  });
});
