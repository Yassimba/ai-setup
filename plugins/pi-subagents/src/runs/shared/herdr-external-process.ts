import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { fileURLToPath } from "node:url";
import type { CommandSpec } from "./external-backends.ts";
import type { ExternalProcessOptions, ExternalProcessResult } from "./external-process.ts";
import { HerdrWorkspaceManager, type HerdrWorkspaceSettings } from "./herdr-workspace.ts";

export type HerdrExternalProcessResult = ExternalProcessResult & {
	executionMode: "herdr" | "local-fallback";
	workspaceId?: string;
	paneId?: string;
	finishPane?: (outcome: "success" | "failure") => Promise<void>;
};
const POLL_MS = 400;

export async function runHerdrExternalProcess(spec: CommandSpec, options: ExternalProcessOptions & { settings: HerdrWorkspaceSettings; label: string }): Promise<HerdrExternalProcessResult | undefined> {
	if (options.signal?.aborted) return { stdout: "", stderr: "", exitCode: null, signal: null, elapsedMs: 0, timedOut: false, cancelled: true, error: "External command cancelled before start.", executionMode: "herdr" };
	const dir = fs.mkdtempSync(path.join(os.tmpdir(), "pi-subagents-herdr-")); fs.chmodSync(dir, 0o700);
	const envelopePath = path.join(dir, "envelope.json"), resultPath = path.join(dir, "result.json");
	fs.writeFileSync(envelopePath, JSON.stringify({ spec, cwd: options.cwd, resultPath }), { encoding: "utf8", mode: 0o600 });
	fs.chmodSync(envelopePath, 0o600);
	const manager = new HerdrWorkspaceManager(options.settings);
	const helper = fileURLToPath(new URL("./external-pane-runner.ts", import.meta.url));
	const pane = await manager.startPane({ label: options.label, cwd: options.cwd, command: process.execPath, args: ["--experimental-strip-types", helper, envelopePath] });
	if (!pane) { fs.rmSync(dir, { recursive: true, force: true }); return undefined; }
	const startedAt = Date.now(); let timedOut = false, cancelled = options.signal?.aborted ?? false;
	const abort = () => { cancelled = true; };
	options.signal?.addEventListener("abort", abort, { once: true });
	try {
		for (;;) {
			if (options.timeoutMs !== undefined && Date.now() - startedAt >= options.timeoutMs) timedOut = true;
			if (cancelled || timedOut) {
				await manager.interruptPane(pane);
				return { stdout: "", stderr: "", exitCode: null, signal: "SIGTERM", elapsedMs: Date.now() - startedAt, timedOut, cancelled, error: timedOut ? `External command timed out after ${options.timeoutMs}ms.` : "External command cancelled.", executionMode: "herdr", workspaceId: pane.workspaceId, paneId: pane.paneId };
			}
			try {
				const result = JSON.parse(fs.readFileSync(resultPath, "utf8")) as ExternalProcessResult;
				return {
					...result,
					executionMode: "herdr",
					workspaceId: pane.workspaceId,
					paneId: pane.paneId,
					finishPane: (outcome) => manager.finishPane(pane, outcome),
				};
			} catch {}
			if (!(await manager.isPaneAlive(pane))) return { stdout: "", stderr: "", exitCode: 1, signal: null, elapsedMs: Date.now() - startedAt, timedOut: false, cancelled: false, error: "Herdr pane ended before writing an external result.", executionMode: "herdr", workspaceId: pane.workspaceId, paneId: pane.paneId };
			await new Promise((resolve) => setTimeout(resolve, POLL_MS));
		}
	} finally { options.signal?.removeEventListener("abort", abort); fs.rmSync(dir, { recursive: true, force: true }); }
}
