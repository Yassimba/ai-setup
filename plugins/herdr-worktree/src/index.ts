import { spawn } from "node:child_process";
import { mkdtemp, readFile, realpath, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { SessionManager } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

type HerdrWorktreeResult = {
  result?: { pane?: { pane_id?: string } };
  error?: { code?: string; message?: string };
};

type WtSwitchResult = {
  path?: string;
  branch?: string;
};

type WtListEntry = {
  branch?: string | null;
  path?: string;
  kind?: string;
};

type StartOptions = {
  branch?: string;
  base?: string;
  sourceCheckout?: string;
  closeOldPane: boolean;
};

type SessionSnapshot = {
  file: string;
  cleanup: () => Promise<void>;
};

export default function (pi: ExtensionAPI) {
  pi.registerTool({
    name: "herdr_start_worktree",
    label: "Start Herdr Worktree",
    description:
      "Create a git worktree through worktrunk (wt) from the current repo checkout — running its " +
      "lifecycle hooks — split the current Herdr pane, continue the active pi session in the new " +
      "sibling pane, then shut down and clean up the old pane.",
    promptSnippet: "Continue the active pi session in a worktree pane in the current Herdr tab",
    promptGuidelines: [
      "Use herdr_start_worktree when work should continue in a fresh git worktree.",
      "herdr_start_worktree creates the checkout via worktrunk (wt switch --create), so worktrunk's " +
        "worktree-path template and post-start hooks apply. It branches from the checkout's current " +
        "branch unless a base is given.",
      "ALWAYS pass a branch name, as a slug of the form {project-name}-{feature-name} " +
        "(e.g. herdr-pane-resize). Derive the feature name from the task at hand.",
      "After herdr_start_worktree succeeds, the current pi process will shut down and the old Herdr pane will close.",
    ],
    parameters: Type.Object({
      branch: Type.Optional(
        Type.String({
          description:
            "Branch name for the new worktree. ALWAYS provide this, as a slug of the form " +
            "{project-name}-{feature-name} (e.g. herdr-pane-resize). If omitted, a " +
            "worktree/{project-name}-{day}-{month} branch is generated as a last resort. " +
            "An existing branch is switched to instead of created.",
        }),
      ),
      base: Type.Optional(
        Type.String({
          description:
            "Base branch for the new worktree (worktrunk shortcuts like ^ work). When omitted, " +
            "the source checkout's current branch is used.",
        }),
      ),
      closeOldPane: Type.Optional(
        Type.Boolean({
          description: "Close the old Herdr pane after the old pi process exits. Defaults to true.",
        }),
      ),
    }),
    async execute(_toolCallId, params, signal, _onUpdate, ctx) {
      return startHerdrWorktree(pi, ctx, signal, {
        branch: cleanOptional(params.branch),
        base: cleanOptional(params.base),
        closeOldPane: params.closeOldPane ?? true,
      });
    },
  });

  pi.registerCommand("herdr-worktree-start", {
    description:
      "Create a Herdr worktree, continue this pi session in it, and clean up the old pane",
    handler: async (args, ctx) => {
      await ctx.waitForIdle();
      try {
        const parsed = parseCommandArgs(args ?? "");
        const result = await startHerdrWorktree(pi, ctx, undefined, parsed);
        const text =
          result.content?.[0]?.type === "text" ? result.content[0].text : "Started worktree";
        ctx.ui.notify(text, "info");
      } catch (err) {
        ctx.ui.notify(errorMessage(err), "error");
      }
    },
  });
}

async function startHerdrWorktree(
  pi: ExtensionAPI,
  ctx: ExtensionContext,
  signal: AbortSignal | undefined,
  options: StartOptions,
) {
  if (process.env.HERDR_ENV !== "1") {
    throw new Error("herdr_start_worktree must run inside a Herdr-managed pane");
  }

  const oldPaneId = process.env.HERDR_PANE_ID;
  if (!oldPaneId) {
    throw new Error("HERDR_PANE_ID is missing; cannot split the current Herdr pane safely");
  }

  const currentFile = ctx.sessionManager.getSessionFile();
  if (!currentFile) {
    throw new Error("Current pi session is not persisted, so it cannot be continued in a worktree");
  }

  const sourceCheckout = await canonicalDirectory(options.sourceCheckout || process.cwd());
  ctx.ui.setStatus("herdr-worktree", "creating worktree");

  let created: Awaited<ReturnType<typeof createHerdrWorktree>> | undefined;
  let newSessionFile: string | undefined;
  let replacementStarted = false;
  let snapshot: SessionSnapshot | undefined;
  try {
    // Pi assigns a session filename before it persists the file. Snapshot the in-memory
    // state first so a command invoked in a fresh session can still be continued, and so
    // session validation happens before creating a worktree with external side effects.
    snapshot = await createSessionSnapshot(ctx.sessionManager);

    created = await createHerdrWorktree(pi, signal, sourceCheckout, options, oldPaneId);
    const worktreePath = await canonicalDirectory(created.worktreePath);

    newSessionFile = await forkSessionFile(snapshot.file, worktreePath);

    await runInNewPane(pi, signal, created.rootPaneId, newSessionFile, worktreePath);
    replacementStarted = true;

    if (options.closeOldPane && oldPaneId) {
      await scheduleOldPaneCleanup(pi, signal, currentFile, oldPaneId, process.pid);
    }

    ctx.ui.setStatus("herdr-worktree", undefined);
    ctx.ui.notify(`Started pi in Herdr worktree: ${worktreePath}`, "info");
    ctx.shutdown();

    return {
      content: [
        {
          type: "text" as const,
          text:
            `Started replacement pi in Herdr worktree: ${worktreePath}\n` +
            `Pane: ${created.rootPaneId} (same Herdr tab)\n` +
            `Branch: ${created.branch ?? "unknown"}\n\n` +
            "The old pi process is shutting down. The old pane will close after it exits.",
        },
      ],
      details: {
        worktreePath,
        branch: created.branch,
        paneId: created.rootPaneId,
        newSessionFile,
        oldSessionFile: currentFile,
        oldPaneId,
      },
      terminate: true,
    };
  } catch (err) {
    ctx.ui.setStatus("herdr-worktree", undefined);
    if (created && !replacementStarted) {
      await herdr(pi, ["pane", "close", created.rootPaneId], signal, undefined, 10_000).catch(
        () => undefined,
      );
    }
    if (newSessionFile) {
      await rm(newSessionFile, { force: true }).catch(() => undefined);
    }
    throw err;
  } finally {
    await snapshot?.cleanup().catch(() => undefined);
  }
}

export async function createHerdrWorktree(
  pi: ExtensionAPI,
  signal: AbortSignal | undefined,
  sourceCheckout: string,
  options: StartOptions,
  sourcePaneId: string,
): Promise<{
  worktreePath: string;
  branch: string;
  rootPaneId: string;
}> {
  const branch = options.branch ?? generateBranchName(sourceCheckout);
  const worktreePath = await worktrunkSwitch(pi, signal, sourceCheckout, branch, options.base);

  const json = await herdrJson(
    pi,
    ["pane", "split", sourcePaneId, "--direction", "right"],
    signal,
    sourceCheckout,
    10_000,
  );
  const rootPaneId = json.result?.pane?.pane_id;
  if (!rootPaneId) {
    throw new Error("Herdr pane split response did not include pane.pane_id");
  }

  return { worktreePath, rootPaneId, branch };
}

/**
 * Create (or reuse) the worktree through worktrunk so its worktree-path template and
 * lifecycle hooks apply. Returns the checkout path.
 */
async function worktrunkSwitch(
  pi: ExtensionAPI,
  signal: AbortSignal | undefined,
  sourceCheckout: string,
  branch: string,
  base: string | undefined,
): Promise<string> {
  const wt = await worktrunkBin();
  const exists = await branchExists(pi, signal, sourceCheckout, branch);
  const args = ["switch"];
  // An existing branch is switched to (wt creates its worktree if missing); base only
  // applies on create, where the default is the source checkout's current branch ("@").
  if (exists) args.push(branch);
  else args.push("--create", branch, "--base", base ?? "@");
  args.push("--no-cd", "--format=json");

  // Generous timeout: wt runs post-start hooks (dependency installs, etc.) inline.
  const result = await pi.exec(wt, args, { cwd: sourceCheckout, signal, timeout: 300_000 });
  if (result.code !== 0) {
    throw new Error(`wt ${args.join(" ")} failed: ${result.stderr || result.stdout}`);
  }

  const parsed = parseJson<WtSwitchResult>(result.stdout.trim());
  if (parsed?.path) return parsed.path;

  // Older worktrunk versions print no JSON for some switch paths; recover via wt list.
  const listResult = await pi.exec(wt, ["list", "--format=json"], {
    cwd: sourceCheckout,
    signal,
    timeout: 30_000,
  });
  const entries = parseJson<WtListEntry[]>(listResult.stdout.trim()) ?? [];
  const entry = entries.find((e) => e.branch === branch && e.kind === "worktree" && e.path);
  if (!entry?.path) {
    throw new Error(`worktrunk returned no worktree path for branch: ${branch}`);
  }
  return entry.path;
}

let cachedWorktrunkBin: string | undefined;

async function worktrunkBin(): Promise<string> {
  cachedWorktrunkBin ??= await resolveWorktrunkBin(process.env, process.platform);
  return cachedWorktrunkBin;
}

/**
 * Resolve the worktrunk executable. On Windows a bare `wt` hits Windows Terminal's
 * app-execution alias (%LOCALAPPDATA%\Microsoft\WindowsApps\wt.exe), which opens a
 * terminal window instead of failing cleanly, so scan PATH ourselves with PATHEXT-aware
 * candidates and skip that alias directory. WORKTRUNK_BIN overrides the search.
 */
export async function resolveWorktrunkBin(
  env: Record<string, string | undefined>,
  platform: NodeJS.Platform,
): Promise<string> {
  const override = cleanOptional(env.WORKTRUNK_BIN);
  if (override) return override;
  if (platform !== "win32") return "wt";

  const extensions = (env.PATHEXT || ".COM;.EXE;.BAT;.CMD").split(";").filter(Boolean);
  let sawWindowsTerminalAlias = false;
  for (const dir of (env.PATH ?? "").split(";").filter(Boolean)) {
    const isAliasDir = dir.replace(/\//g, "\\").toLowerCase().includes("\\microsoft\\windowsapps");
    for (const extension of extensions) {
      const candidate = join(dir, `wt${extension}`);
      const s = await stat(candidate).catch(() => undefined);
      if (!s?.isFile()) continue;
      if (isAliasDir) {
        sawWindowsTerminalAlias = true;
        break;
      }
      return candidate;
    }
  }

  throw new Error(
    sawWindowsTerminalAlias
      ? "worktrunk (wt) is not installed — only Windows Terminal's wt.exe was found on PATH. " +
          "Install worktrunk or set WORKTRUNK_BIN to its full path."
      : "worktrunk (wt) was not found on PATH. Install worktrunk or set WORKTRUNK_BIN to its full path.",
  );
}

async function branchExists(
  pi: ExtensionAPI,
  signal: AbortSignal | undefined,
  cwd: string,
  branch: string,
): Promise<boolean> {
  const result = await pi.exec("git", ["show-ref", "--verify", "--quiet", `refs/heads/${branch}`], {
    cwd,
    signal,
    timeout: 10_000,
  });
  return result.code === 0;
}

function generateBranchName(sourceCheckout: string): string {
  const project = basename(sourceCheckout);
  const now = new Date();
  const day = String(now.getDate()).padStart(2, "0");
  const month = String(now.getMonth() + 1).padStart(2, "0");
  return `worktree/${project}-${day}-${month}`;
}

function parseJson<T>(raw: string): T | undefined {
  try {
    return JSON.parse(raw) as T;
  } catch {
    return undefined;
  }
}

async function runInNewPane(
  pi: ExtensionAPI,
  signal: AbortSignal | undefined,
  paneId: string,
  sessionFile: string,
  worktreePath: string,
): Promise<void> {
  const command = buildPaneRunCommand(sessionFile, worktreePath);
  await herdr(pi, ["pane", "run", paneId, command], signal, undefined, 10_000);
}

/**
 * Build the one-liner typed into the new pane. Herdr's default pane shell on Windows is
 * powershell.exe, which does not parse POSIX `cd ... && exec ...`, so synthesize a
 * PowerShell statement there and keep the POSIX form everywhere else.
 */
export function buildPaneRunCommand(
  sessionFile: string,
  worktreePath: string,
  platform: NodeJS.Platform = process.platform,
): string {
  const continuation = `Moved to worktree ${worktreePath}. Continue.`;
  if (platform === "win32") {
    const piArgs = ["--session", sessionFile, continuation].map(powershellQuote).join(" ");
    return `Set-Location -LiteralPath ${powershellQuote(worktreePath)} -ErrorAction Stop; & pi ${piArgs}`;
  }
  const piCommand = ["pi", "--session", sessionFile, continuation].map(posixShellQuote).join(" ");
  return `cd ${posixShellQuote(worktreePath)} && exec ${piCommand}`;
}

async function scheduleOldPaneCleanup(
  pi: ExtensionAPI,
  signal: AbortSignal | undefined,
  oldSessionFile: string,
  oldPaneId: string,
  oldPid: number,
): Promise<void> {
  if (process.platform === "win32") {
    await spawnWindowsPaneCleanupWatcher(oldSessionFile, oldPaneId, oldPid);
    return;
  }

  const cleanup = [
    `old_pid=${oldPid}`,
    `old_session=${posixShellQuote(oldSessionFile)}`,
    `old_pane=${posixShellQuote(oldPaneId)}`,
    "i=0",
    'while kill -0 "$old_pid" 2>/dev/null && [ "$i" -lt 600 ]; do i=$((i + 1)); sleep 0.1; done',
    'rm -f -- "$old_session"',
    'herdr pane close "$old_pane" >/dev/null 2>&1 || true',
  ].join("; ");

  const launcher =
    "if command -v setsid >/dev/null 2>&1; then " +
    `setsid sh -c ${posixShellQuote(cleanup)} >/dev/null 2>&1 < /dev/null & ` +
    "else " +
    `nohup sh -c ${posixShellQuote(cleanup)} >/dev/null 2>&1 < /dev/null & ` +
    "fi";

  const result = await pi.exec("sh", ["-lc", launcher], { signal, timeout: 5_000 });
  if (result.code !== 0) {
    throw new Error(`Failed to schedule old pane cleanup: ${result.stderr || result.stdout}`);
  }
}

/**
 * Node source for the detached Windows cleanup watcher. It mirrors the sh script above:
 * poll the old pid (up to 600 x 100ms), remove the old session file, then close the old
 * pane with `herdr pane close` (resolved from PATH via the inherited env, like the sh
 * branch), ignoring failures. Inputs arrive via argv, not source interpolation.
 */
const PANE_CLEANUP_WATCHER_SOURCE = [
  'const { spawnSync } = require("node:child_process");',
  'const { rmSync } = require("node:fs");',
  "const [pidRaw, sessionFile, paneId] = process.argv.slice(1);",
  "const pid = Number(pidRaw);",
  "const alive = () => { try { process.kill(pid, 0); return true; } catch { return false; } };",
  "(async () => {",
  "  for (let i = 0; i < 600 && alive(); i += 1) {",
  "    await new Promise((done) => setTimeout(done, 100));",
  "  }",
  "  try { rmSync(sessionFile, { force: true }); } catch {}",
  '  spawnSync("herdr", ["pane", "close", paneId], { stdio: "ignore" });',
  "})();",
].join("\n");

export function paneCleanupWatcherArgs(
  oldSessionFile: string,
  oldPaneId: string,
  oldPid: number,
): string[] {
  return ["-e", PANE_CLEANUP_WATCHER_SOURCE, String(oldPid), oldSessionFile, oldPaneId];
}

/**
 * Windows has no sh/setsid/nohup, so detach a Node child as the watcher instead
 * (process.kill(pid, 0) works for liveness polling on Windows).
 */
async function spawnWindowsPaneCleanupWatcher(
  oldSessionFile: string,
  oldPaneId: string,
  oldPid: number,
): Promise<void> {
  await new Promise<void>((done, reject) => {
    const child = spawn(
      process.execPath,
      paneCleanupWatcherArgs(oldSessionFile, oldPaneId, oldPid),
      {
        detached: true,
        stdio: "ignore",
        windowsHide: true,
      },
    );
    child.once("error", (err) => {
      reject(new Error(`Failed to schedule old pane cleanup: ${err.message}`));
    });
    child.once("spawn", () => {
      child.unref();
      done();
    });
  });
}

export async function createSessionSnapshot(
  sessionManager: Pick<SessionManager, "getEntries" | "getHeader">,
): Promise<SessionSnapshot> {
  const header = sessionManager.getHeader();
  if (!header) {
    throw new Error("Current pi session has no valid in-memory header");
  }

  const directory = await mkdtemp(join(tmpdir(), "pi-herdr-worktree-session-"));
  const file = join(directory, "session.jsonl");
  const entries = [header, ...sessionManager.getEntries()];

  try {
    await writeFile(file, `${entries.map((entry) => JSON.stringify(entry)).join("\n")}\n`, "utf8");
  } catch (error) {
    await rm(directory, { recursive: true, force: true }).catch(() => undefined);
    throw error;
  }

  return {
    file,
    cleanup: () => rm(directory, { recursive: true, force: true }),
  };
}

export async function forkSessionFile(
  sourceSessionFile: string,
  worktreePath: string,
): Promise<string> {
  const forked = SessionManager.forkFrom(sourceSessionFile, worktreePath);
  const newFile = forked.getSessionFile();
  if (!newFile) {
    throw new Error("Failed to create forked session file for the new worktree");
  }

  const raw = await readFile(newFile, "utf8");
  const lines = raw.trimEnd().split("\n");
  if (lines.length > 0 && lines[0]) {
    const header = JSON.parse(lines[0]);
    if (header.parentSession !== undefined) {
      delete header.parentSession;
      lines[0] = JSON.stringify(header);
      await writeFile(newFile, `${lines.join("\n")}\n`, "utf8");
    }
  }

  return newFile;
}

async function canonicalDirectory(path: string): Promise<string> {
  const resolved = resolve(path.replace(/^@/, ""));
  const s = await stat(resolved).catch(() => undefined);
  if (!s?.isDirectory()) {
    throw new Error(`Directory does not exist: ${resolved}`);
  }
  return realpath(resolved);
}

async function herdrJson(
  pi: ExtensionAPI,
  args: string[],
  signal: AbortSignal | undefined,
  cwd: string | undefined,
  timeout: number,
): Promise<HerdrWorktreeResult> {
  const result = await herdr(pi, args, signal, cwd, timeout);
  const raw = result.stdout.trim() || result.stderr.trim();
  let json: HerdrWorktreeResult;
  try {
    json = JSON.parse(raw) as HerdrWorktreeResult;
  } catch {
    throw new Error(`Herdr returned non-JSON output for ${args.join(" ")}: ${raw}`);
  }
  if (json.error) {
    throw new Error(
      `${json.error.code ?? "herdr_error"}: ${json.error.message ?? "unknown Herdr error"}`,
    );
  }
  return json;
}

async function herdr(
  pi: ExtensionAPI,
  args: string[],
  signal: AbortSignal | undefined,
  cwd: string | undefined,
  timeout: number,
) {
  const result = await pi.exec("herdr", args, { cwd, signal, timeout });
  if (result.code !== 0) {
    throw new Error(`herdr ${args.join(" ")} failed: ${result.stderr || result.stdout}`);
  }
  return result;
}

function parseCommandArgs(args: string): StartOptions {
  const tokens = tokenize(args);
  const options: StartOptions = {
    closeOldPane: true,
  };

  for (let i = 0; i < tokens.length; i += 1) {
    const token = tokens[i];
    if (token === "--branch") options.branch = requireValue(tokens, ++i, token);
    else if (token === "--base") options.base = requireValue(tokens, ++i, token);
    else if (token === "--source") options.sourceCheckout = requireValue(tokens, ++i, token);
    else if (token === "--no-close-pane") options.closeOldPane = false;
    else if (!options.branch) options.branch = token;
    else throw new Error(`Unexpected argument: ${token}`);
  }

  options.branch = cleanOptional(options.branch);
  options.base = cleanOptional(options.base);
  options.sourceCheckout = cleanOptional(options.sourceCheckout);
  return options;
}

type TokenizerState = {
  tokens: string[];
  current: string;
  quote?: '"' | "'";
  escaping: boolean;
};

export function tokenize(input: string, platform: NodeJS.Platform = process.platform): string[] {
  // On Windows, backslash is the path separator (C:\repos\foo), not an escape character.
  const backslashEscapes = platform !== "win32";
  const state: TokenizerState = { tokens: [], current: "", escaping: false };
  for (const ch of input.trim()) {
    consumeChar(state, ch, backslashEscapes);
  }
  if (state.quote) throw new Error("Unclosed quote in command arguments");
  if (state.escaping) state.current += "\\";
  flushToken(state);
  return state.tokens;
}

function consumeChar(state: TokenizerState, ch: string, backslashEscapes: boolean): void {
  if (state.escaping) {
    state.current += ch;
    state.escaping = false;
    return;
  }
  if (backslashEscapes && ch === "\\") {
    state.escaping = true;
    return;
  }
  if (state.quote) {
    if (ch === state.quote) state.quote = undefined;
    else state.current += ch;
    return;
  }
  if (ch === '"' || ch === "'") {
    state.quote = ch;
    return;
  }
  if (/\s/.test(ch)) {
    flushToken(state);
    return;
  }
  state.current += ch;
}

function flushToken(state: TokenizerState): void {
  if (state.current) {
    state.tokens.push(state.current);
    state.current = "";
  }
}

function requireValue(tokens: string[], index: number, flag: string): string {
  const value = tokens[index];
  if (!value || value.startsWith("--")) {
    throw new Error(`Missing value for ${flag}`);
  }
  return value;
}

function cleanOptional(value: unknown): string | undefined {
  if (typeof value !== "string") return undefined;
  const trimmed = value.trim();
  return trimmed ? trimmed : undefined;
}

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export function posixShellQuote(value: string): string {
  return `'${value.replace(/'/g, `'"'"'`)}'`;
}

/** PowerShell single-quoted literal: the only escape is doubling embedded quotes. */
export function powershellQuote(value: string): string {
  return `'${value.replace(/'/g, "''")}'`;
}
