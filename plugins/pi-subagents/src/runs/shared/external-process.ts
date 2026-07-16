import { spawn } from "node:child_process";
import { basename } from "node:path";
import { toSpawnableCommand } from "./executable-resolver.ts";
import type { CommandSpec } from "./external-backends.ts";
import { attachPostExitStdioGuard, trySignalChild } from "../../shared/post-exit-stdio-guard.ts";

export interface ExternalProcessOptions {
	cwd: string;
	stdin?: string;
	prompt?: string;
	promptMode?: "stdin" | "argument";
	signal?: AbortSignal;
	timeoutMs?: number;
	killGraceMs?: number;
}

export interface ExternalProcessResult {
	stdout: string;
	stderr: string;
	exitCode: number | null;
	signal: NodeJS.Signals | null;
	elapsedMs: number;
	timedOut: boolean;
	cancelled: boolean;
	error?: string;
}

function safeCommandName(command: string): string {
	return basename(command) || "external command";
}

export function runExternalProcess(spec: CommandSpec, options: ExternalProcessOptions): Promise<ExternalProcessResult> {
	const startedAt = Date.now();
	if (options.signal?.aborted) return Promise.resolve({
		stdout: "", stderr: "", exitCode: null, signal: null, elapsedMs: 0,
		timedOut: false, cancelled: true, error: "External command cancelled before start.",
	});

	return new Promise((resolve) => {
		const promptMode = options.promptMode ?? "stdin";
		const args = [...spec.args];
		if (options.prompt !== undefined && promptMode === "argument") args.push(options.prompt);
		let child;
		try {
			// Windows .cmd/.bat shims run through an escaped cmd.exe command line.
			const spawnable = toSpawnableCommand(spec.command, args);
			child = spawn(spawnable.command, spawnable.args, {
				cwd: options.cwd,
				env: spec.env ? { ...process.env, ...spec.env } : process.env,
				shell: false,
				...(spawnable.windowsVerbatimArguments ? { windowsVerbatimArguments: true } : {}),
				stdio: ["pipe", "pipe", "pipe"],
				detached: process.platform !== "win32",
				windowsHide: true,
			});
		} catch (error) {
			resolve({ stdout: "", stderr: "", exitCode: null, signal: null, elapsedMs: Date.now() - startedAt, timedOut: false, cancelled: false, error: `Unable to start ${safeCommandName(spec.command)}: ${error instanceof Error ? error.message : "unknown spawn error"}` });
			return;
		}

		let stdout = "";
		let stderr = "";
		let timedOut = false;
		let cancelled = false;
		let settled = false;
		let timeout: NodeJS.Timeout | undefined;
		let forceTimer: NodeJS.Timeout | undefined;
		const clearStdioGuard = attachPostExitStdioGuard(child, { idleMs: 100, hardMs: 1_000 });
		child.stdout?.setEncoding("utf8");
		child.stderr?.setEncoding("utf8");
		child.stdout?.on("data", (chunk: string) => { stdout += chunk; });
		child.stderr?.on("data", (chunk: string) => { stderr += chunk; });

		const signalTree = (signal: NodeJS.Signals) => {
			if (process.platform === "win32" && child.pid) {
				try {
					const killer = spawn("taskkill", ["/pid", String(child.pid), "/T", ...(signal === "SIGKILL" ? ["/F"] : [])], {
						stdio: "ignore", shell: false, windowsHide: true,
					});
					killer.unref();
					return true;
				} catch {}
			} else if (child.pid) {
				try { process.kill(-child.pid, signal); return true; } catch {}
			}
			return trySignalChild(child, signal);
		};
		const terminate = () => {
			signalTree("SIGTERM");
			forceTimer = setTimeout(() => signalTree("SIGKILL"), options.killGraceMs ?? 250);
			forceTimer.unref?.();
		};
		const onAbort = () => { if (!settled && !timedOut) { cancelled = true; terminate(); } };
		options.signal?.addEventListener("abort", onAbort, { once: true });
		if (options.timeoutMs !== undefined) {
			timeout = setTimeout(() => { if (!settled && !cancelled) { timedOut = true; terminate(); } }, Math.max(0, options.timeoutMs));
			timeout.unref?.();
		}

		child.on("error", (error) => {
			if (settled) return;
			settled = true;
			cleanup();
			resolve({ stdout, stderr, exitCode: null, signal: null, elapsedMs: Date.now() - startedAt, timedOut, cancelled, error: `Unable to run ${safeCommandName(spec.command)}: ${error.message}` });
		});
		child.on("close", (exitCode, signal) => {
			if (settled) return;
			settled = true;
			cleanup();
			resolve({ stdout, stderr, exitCode, signal, elapsedMs: Date.now() - startedAt, timedOut, cancelled,
				error: timedOut ? `External command timed out after ${options.timeoutMs}ms.` : cancelled ? "External command cancelled." : exitCode && exitCode !== 0 ? `External command exited with code ${exitCode}.` : undefined });
		});

		function cleanup() {
			if (timeout) clearTimeout(timeout);
			if (forceTimer) clearTimeout(forceTimer);
			options.signal?.removeEventListener("abort", onAbort);
			clearStdioGuard();
		}

		child.stdin?.on("error", () => {});
		const input = options.stdin ?? (options.prompt !== undefined && promptMode === "stdin" ? options.prompt : undefined);
		if (input !== undefined) child.stdin?.end(input);
		else child.stdin?.end();
	});
}

export const executeExternalProcess = runExternalProcess;
