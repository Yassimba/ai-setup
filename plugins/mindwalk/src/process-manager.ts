import { type ChildProcess, spawn } from "node:child_process";

const SERVING_URL = /mindwalk serving (http:\/\/127\.0\.0\.1:\d+\/[^\s]*)/;

export interface MindwalkOpenResult {
  url: string;
  reused: boolean;
  browserOpened: boolean;
}

export type OpenBrowser = (url: string) => Promise<boolean>;
export type SpawnMindwalk = typeof spawn;

export interface MindwalkProcessManagerOptions {
  binaryPath: string;
  openBrowser: OpenBrowser;
  spawnProcess?: SpawnMindwalk;
  startupTimeoutMs?: number;
}

export class MindwalkProcessManager {
  readonly #binaryPath: string;
  readonly #openBrowser: OpenBrowser;
  readonly #spawnProcess: SpawnMindwalk;
  readonly #startupTimeoutMs: number;
  #child: ChildProcess | undefined;
  #sessionFile: string | undefined;
  #url: string | undefined;

  constructor(options: MindwalkProcessManagerOptions) {
    this.#binaryPath = options.binaryPath;
    this.#openBrowser = options.openBrowser;
    this.#spawnProcess = options.spawnProcess ?? spawn;
    this.#startupTimeoutMs = options.startupTimeoutMs ?? 10_000;
  }

  currentUrl(): string | undefined {
    return this.#url;
  }

  async open(sessionFile: string, cwd: string): Promise<MindwalkOpenResult> {
    if (this.#sessionFile === sessionFile && this.#url && this.#isRunning()) {
      return {
        url: this.#url,
        reused: true,
        browserOpened: await this.#openBrowser(this.#url),
      };
    }

    await this.stop();
    const child = this.#spawnProcess(this.#binaryPath, ["open", "--no-open", sessionFile], {
      cwd,
      stdio: ["ignore", "pipe", "pipe"],
      windowsHide: true,
    });
    this.#child = child;
    this.#sessionFile = sessionFile;
    child.once("exit", () => {
      if (this.#child !== child) return;
      this.#child = undefined;
      this.#sessionFile = undefined;
      this.#url = undefined;
    });

    const url = await this.#waitForUrl(child);
    if (this.#child !== child) throw new Error("Mindwalk stopped during startup");
    this.#url = url;
    return {
      url,
      reused: false,
      browserOpened: await this.#openBrowser(url),
    };
  }

  async stop(): Promise<void> {
    const child = this.#child;
    this.#child = undefined;
    this.#sessionFile = undefined;
    this.#url = undefined;
    if (!child || child.exitCode !== null) return;

    child.kill("SIGTERM");
    if (await waitForExit(child, 1_000)) return;
    child.kill("SIGKILL");
    await waitForExit(child, 1_000);
  }

  #isRunning(): boolean {
    return this.#child !== undefined && this.#child.exitCode === null;
  }

  #waitForUrl(child: ChildProcess): Promise<string> {
    return new Promise((resolve, reject) => {
      let stdout = "";
      let stderr = "";
      const timer = setTimeout(() => {
        cleanup();
        child.kill("SIGTERM");
        reject(
          new Error(
            `Mindwalk did not report a local URL within ${this.#startupTimeoutMs}ms${formatStderr(stderr)}`,
          ),
        );
      }, this.#startupTimeoutMs);

      const onStdout = (chunk: Buffer | string) => {
        stdout = `${stdout}${chunk}`.slice(-16_384);
        const match = stdout.match(SERVING_URL);
        if (!match?.[1]) return;
        cleanup();
        resolve(match[1]);
      };
      const onStderr = (chunk: Buffer | string) => {
        stderr = `${stderr}${chunk}`.slice(-4_096);
      };
      const onError = (error: Error) => {
        cleanup();
        reject(error);
      };
      const onExit = (code: number | null, signal: NodeJS.Signals | null) => {
        cleanup();
        reject(
          new Error(
            `Mindwalk exited before startup (code ${code ?? "none"}, signal ${signal ?? "none"})${formatStderr(stderr)}`,
          ),
        );
      };
      const cleanup = () => {
        clearTimeout(timer);
        child.stdout?.off("data", onStdout);
        child.stderr?.off("data", onStderr);
        child.off("error", onError);
        child.off("exit", onExit);
      };

      child.stdout?.on("data", onStdout);
      child.stderr?.on("data", onStderr);
      child.once("error", onError);
      child.once("exit", onExit);
    });
  }
}

function formatStderr(stderr: string): string {
  const detail = stderr.trim();
  return detail ? `: ${detail}` : "";
}

function waitForExit(child: ChildProcess, timeoutMs: number): Promise<boolean> {
  if (child.exitCode !== null) return Promise.resolve(true);
  return new Promise((resolve) => {
    const timer = setTimeout(() => {
      child.off("exit", onExit);
      resolve(false);
    }, timeoutMs);
    const onExit = () => {
      clearTimeout(timer);
      resolve(true);
    };
    child.once("exit", onExit);
  });
}
