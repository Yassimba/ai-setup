// biome-ignore-all lint/suspicious/noControlCharactersInRegex: ANSI escape sequences are the behavior under test.
import assert from "node:assert/strict";
import { test } from "node:test";
import { FastFooter, fastColorToAnsi, renderFastLabel } from "../plugins/openai-fast/src/footer.ts";

const ANSI = { dim: "\x1b[38;5;8m", error: "\x1b[31m", warning: "\x1b[33m" };
const THINKING_ANSI = { low: "\x1b[38;5;75m", high: "\x1b[38;5;147m" };

function createTheme(options = {}) {
  return {
    name: options.name,
    fg: (color, text) => `${ANSI[color] ?? ""}${text}\x1b[39m`,
    getColorMode: options.getColorMode,
    getThinkingBorderColor: (level) => (text) => `${THINKING_ANSI[level] ?? ""}${text}\x1b[39m`,
  };
}

function createFooter(options = {}) {
  const context = {
    model: Object.hasOwn(options, "model")
      ? options.model
      : { provider: "partner", id: "gpt-5.5", reasoning: true, contextWindow: 200000 },
    sessionManager: {
      getCwd: () => options.cwd ?? "/work/repo",
      getSessionName: () => options.sessionName,
      getEntries: () => options.entries ?? [],
    },
    modelRegistry: { isUsingOAuth: () => options.usingOAuth ?? false },
    getContextUsage: () => options.contextUsage ?? { percent: 10, contextWindow: 200000 },
  };
  return new FastFooter({
    getContext: () => context,
    footerData: {
      getGitBranch: () => options.gitBranch ?? null,
      getExtensionStatuses: () => new Map(Object.entries(options.statuses ?? {})),
      getAvailableProviderCount: () => options.providerCount ?? 1,
      onBranchChange: options.onBranchChange,
    },
    theme: options.theme ?? createTheme(),
    isFastActive: () => options.active ?? true,
    getThinkingLevel: () => options.thinkingLevel ?? "off",
    fastLabelColors: options.colors,
    tui: options.tui,
  });
}

test("converts color tokens to ANSI sequences", () => {
  assert.equal(fastColorToAnsi("", "256color"), "\x1b[39m");
  assert.equal(fastColorToAnsi(42, "256color"), "\x1b[38;5;42m");
  assert.equal(fastColorToAnsi("42", "256color"), "\x1b[38;5;42m");
  assert.equal(fastColorToAnsi("#00ffaa", "truecolor"), "\x1b[38;2;0;255;170m");
  assert.equal(fastColorToAnsi("#000000", "256color"), "\x1b[38;5;16m");
  assert.equal(fastColorToAnsi("#ffffff", "256color"), "\x1b[38;5;231m");
  assert.equal(
    fastColorToAnsi("#808080", "256color"),
    "\x1b[38;5;244m",
    "near-gray uses grayscale ramp",
  );
});

test("renders the fast label with theme-matched colors by default", () => {
  const theme = createTheme();
  assert.equal(renderFastLabel(theme, "off", { vars: {} }), `${ANSI.dim}fast\x1b[39m`);
  assert.equal(renderFastLabel(theme, "high", { vars: {} }), `${THINKING_ANSI.high}fast\x1b[39m`);
});

test("custom label colors follow the theme variant and resolve variables", () => {
  const colors = { dark: "#00ffaa", light: "brand", vars: { brand: "#0066cc" } };
  const dark = createTheme({ getColorMode: () => "truecolor" });
  assert.equal(renderFastLabel(dark, "off", colors), "\x1b[38;2;0;255;170mfast\x1b[39m");
  const light = createTheme({ name: "Light", getColorMode: () => "truecolor" });
  assert.equal(renderFastLabel(light, "off", colors), "\x1b[38;2;0;102;204mfast\x1b[39m");
});

test("an unresolvable custom color falls back to the theme-matched label", () => {
  const theme = createTheme();
  assert.equal(
    renderFastLabel(theme, "high", { dark: "ghostVar", vars: {} }),
    `${THINKING_ANSI.high}fast\x1b[39m`,
  );
});

test("the fast label keeps its color inside the dim stats line", () => {
  const footer = createFooter({ thinkingLevel: "high" });
  const line = footer.render(120)[1];
  assert.match(line, /gpt-5\.5 .*\x1b\[38;5;147mfast\x1b\[39m\x1b\[38;5;8m • high/);
});

test("inactive Fast Mode renders the plain model label", () => {
  const footer = createFooter({ active: false, thinkingLevel: "xhigh" });
  const line = footer.render(120)[1];
  assert.match(line, /gpt-5\.5 • xhigh/);
  assert.doesNotMatch(line, /fast/);
});

test("renders directory, branch, session name, and extension statuses", () => {
  const footer = createFooter({
    cwd: "/work/repo",
    gitBranch: "main",
    sessionName: "triage",
    statuses: { b: "two\nlines", a: "one" },
  });
  const lines = footer.render(120);
  assert.equal(lines.length, 3);
  assert.match(lines[0], /\/work\/repo \(main\) • triage/);
  assert.equal(lines[2], "one two lines", "statuses sorted by key and sanitized");
});

test("shows the provider prefix only when several providers are available", () => {
  const withOne = createFooter({ thinkingLevel: "off" }).render(120)[1];
  assert.doesNotMatch(withOne, /\(partner\)/);
  const withTwo = createFooter({ providerCount: 2 }).render(120)[1];
  assert.match(withTwo, /\(partner\) gpt-5\.5/);
});

test("aggregates token usage and cost from assistant entries", () => {
  const entries = [
    {
      type: "message",
      message: {
        role: "assistant",
        usage: { input: 1500, output: 500, cacheRead: 12000, cost: { total: 0.5 } },
      },
    },
    { type: "message", message: { role: "user", usage: { input: 999999 } } },
  ];
  const line = createFooter({ entries }).render(160)[1];
  assert.match(line, /↑1\.5k/);
  assert.match(line, /↓500/);
  assert.match(line, /R12k/);
  assert.match(line, /\$0\.500/);
});

test("colors the context usage when it runs hot and handles unknown usage", () => {
  const hot = createFooter({ contextUsage: { percent: 95, contextWindow: 200000 } }).render(160)[1];
  assert.match(hot, /\x1b\[31m95\.0%\/200k \(auto\)\x1b\[39m/);
  const unknown = createFooter({ contextUsage: { percent: null, contextWindow: 200000 } }).render(
    160,
  )[1];
  assert.match(unknown, /\?\/200k \(auto\)/);
});

test("subscribes to branch changes and unsubscribes on dispose", () => {
  let renders = 0;
  let unsubscribed = false;
  let trigger;
  const footer = createFooter({
    tui: { requestRender: () => renders++ },
    onBranchChange: (callback) => {
      trigger = callback;
      return () => {
        unsubscribed = true;
      };
    },
  });
  trigger();
  assert.equal(renders, 1);
  assert.equal(footer.isOwnedByExtension(), true);
  footer.dispose();
  assert.equal(unsubscribed, true);
  assert.equal(footer.isOwnedByExtension(), false);
});

test("renders nothing without a context", () => {
  const footer = new FastFooter({
    getContext: () => undefined,
    footerData: {
      getGitBranch: () => null,
      getExtensionStatuses: () => new Map(),
      getAvailableProviderCount: () => 1,
    },
    theme: createTheme(),
    isFastActive: () => true,
    getThinkingLevel: () => "off",
  });
  assert.deepEqual(footer.render(80), []);
});
