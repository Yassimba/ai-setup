/**
 * Native herdr mode for async subagent children.
 *
 * When `herdrWorkspace` is enabled in the extension config, async children run
 * as FULL interactive pi sessions inside panes of a dedicated (never focused)
 * herdr workspace instead of headless `--mode json -p` processes. The runner
 * keeps every managed feature by tailing the child's session file — session
 * `message` records carry the same Message objects (content parts, usage,
 * stopReason) as json-mode `message_end` events — and synthesizing the
 * equivalent events for transcripts, status, usage, and result extraction.
 *
 * The pane stays open after completion so the run can be inspected and even
 * continued by hand. If herdr is unreachable the caller falls back to the
 * managed spawn path; enabling the config must never break subagents.
 */

import * as fs from "node:fs";
import * as path from "node:path";
import type { Message } from "@earendil-works/pi-ai";
import type { ChildTranscriptWriter } from "../../shared/child-transcript.ts";
import type { Usage } from "../../shared/types.ts";
import { getFinalOutput } from "../../shared/utils.ts";
import { getPiSpawnCommand } from "../shared/pi-spawn.ts";
import { HerdrWorkspaceManager, gridSlotFor, resolveHerdrWorkspaceSetting, type HerdrWorkspaceSettings } from "../shared/herdr-workspace.ts";

export { gridSlotFor, resolveHerdrWorkspaceSetting };
export type HerdrNativeSettings = HerdrWorkspaceSettings;

const SESSION_POLL_MS = 400;
const COMPLETION_DRAIN_MS = 1_500;
const PANE_MISS_TOLERANCE = 3;

/** Strip managed-mode flags and pin the session file, keeping everything else. */
export function toNativeArgs(args: string[], sessionFile: string): string[] {
	const out: string[] = [];
	for (let i = 0; i < args.length; i++) {
		const arg = args[i];
		if (arg === "--mode" || arg === "--session" || arg === "--session-dir") { i += 1; continue; }
		if (arg === "-p" || arg === "--print") continue;
		out.push(arg);
	}
	out.unshift("--session", sessionFile);
	return out;
}

export interface NativeHerdrRunInput {
	args: string[];
	cwd: string;
	sessionFile: string;
	agentName: string;
	runId: string;
	stepIndex: number;
	settings: HerdrNativeSettings;
	env?: Record<string, string | undefined>;
	piPackageRoot?: string;
	piArgv1?: string;
	transcriptWriter?: ChildTranscriptWriter;
	onChildEvent?: (event: Record<string, unknown>) => void;
	appendChildEvent?: (event: Record<string, unknown>) => void;
	registerInterrupt?: (interrupt: (() => void) | undefined) => void;
	registerTimeout?: (interrupt: (() => void) | undefined) => void;
	registerStop?: (stop: (() => void) | undefined) => void;
	timeoutMessage?: string;
	stopMessage?: string;
}

export interface NativeHerdrRunResult {
	stderr: string;
	exitCode: number | null;
	messages: Message[];
	usage: Usage;
	model?: string;
	error?: string;
	finalOutput: string;
	interrupted?: boolean;
	timedOut?: boolean;
	stopped?: boolean;
	/** True when herdr was unreachable and the caller should use the managed path. */
	fallback?: boolean;
}

function emptyUsage(): Usage {
	return { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, cost: 0, turns: 0 };
}

function isTerminalStop(message: Message): boolean {
	if (message.role !== "assistant") return false;
	const stopReason = (message as { stopReason?: string }).stopReason;
	const hasToolCall =
		Array.isArray(message.content) &&
		message.content.some((part) => (part as { type?: string }).type === "toolCall");
	return stopReason === "stop" && !hasToolCall;
}

/** Poll-based JSONL tail that survives the file not existing yet. */
class SessionTail {
	private position = 0;
	private buffer = "";
	private readonly filePath: string;

	constructor(filePath: string) {
		this.filePath = filePath;
		// Skip content from earlier attempts sharing this session, so a
		// model-fallback retry doesn't re-count the previous attempt's messages.
		try {
			this.position = fs.statSync(filePath).size;
		} catch {
			this.position = 0;
		}
	}

	readNew(): string[] {
		let size: number;
		try {
			size = fs.statSync(this.filePath).size;
		} catch {
			return [];
		}
		if (size <= this.position) return [];
		const fd = fs.openSync(this.filePath, "r");
		try {
			const chunk = Buffer.alloc(size - this.position);
			fs.readSync(fd, chunk, 0, chunk.length, this.position);
			this.position = size;
			this.buffer += chunk.toString("utf-8");
		} finally {
			fs.closeSync(fd);
		}
		const lines = this.buffer.split("\n");
		this.buffer = lines.pop() ?? "";
		return lines.filter((line) => line.trim().length > 0);
	}
}

export async function runPiNativeHerdr(input: NativeHerdrRunInput): Promise<NativeHerdrRunResult> {
	const base: NativeHerdrRunResult = {
		stderr: "",
		exitCode: 0,
		messages: [],
		usage: emptyUsage(),
		finalOutput: "",
	};

	const manager = new HerdrWorkspaceManager(input.settings);
	const spawnSpec = getPiSpawnCommand(input.args, {
		...(input.piPackageRoot ? { piPackageRoot: input.piPackageRoot } : {}),
		...(input.piArgv1 ? { argv1: input.piArgv1 } : {}),
	});
	const paneName = `${input.agentName}-${input.runId}-${input.stepIndex}`;
	const pane = await manager.startPane({ label: paneName, cwd: input.cwd, command: spawnSpec.command, args: spawnSpec.args, env: input.env });
	if (!pane) return { ...base, fallback: true };
	const paneId = pane.paneId;

	fs.mkdirSync(path.dirname(input.sessionFile), { recursive: true });
	const tail = new SessionTail(input.sessionFile);
	const result: NativeHerdrRunResult = { ...base };

	let finished = false;
	let terminalStopAt = 0;
	let paneGone = false;
	let paneMissCount = 0;

	const closePane = async () => {
		if (!paneId) return;
		await manager.interruptPane(pane);
	};
	const markPaneDone = async (marker: string) => {
		await manager.finishPane(pane, marker === "✓" ? "success" : "failure");
	};

	const abortRun = (patch: Partial<NativeHerdrRunResult>) => {
		Object.assign(result, patch, { exitCode: 1 });
		void closePane();
	};
	input.registerInterrupt?.(() => abortRun({ interrupted: true }));
	input.registerTimeout?.(() =>
		abortRun({ timedOut: true, error: input.timeoutMessage ?? "Subagent timed out." }),
	);
	input.registerStop?.(() =>
		abortRun({ stopped: true, error: input.stopMessage ?? "Subagent stopped by user." }),
	);

	const applySessionLine = (line: string) => {
		let record: { type?: string; message?: Message };
		try {
			record = JSON.parse(line) as { type?: string; message?: Message };
		} catch {
			return;
		}
		if (record.type !== "message" || !record.message) return;
		const message = record.message;
		const synthetic = { type: "message_end", message };
		input.transcriptWriter?.writeChildEvent(synthetic);
		input.appendChildEvent?.(synthetic);
		input.onChildEvent?.(synthetic);
		result.messages.push(message);
		if (message.role === "assistant") {
			result.usage.turns += 1;
			const usage = (message as { usage?: { input?: number; output?: number; cacheRead?: number; cacheWrite?: number; cost?: { total?: number } } }).usage;
			if (usage) {
				result.usage.input += usage.input ?? 0;
				result.usage.output += usage.output ?? 0;
				result.usage.cacheRead += usage.cacheRead ?? 0;
				result.usage.cacheWrite += usage.cacheWrite ?? 0;
				result.usage.cost += usage.cost?.total ?? 0;
			}
			const model = (message as { model?: string }).model;
			if (!result.model && model) result.model = model;
			if (isTerminalStop(message)) terminalStopAt = Date.now();
		}
	};

	const isPaneAlive = async (): Promise<boolean> => {
		if (await manager.isPaneAlive(pane)) {
			paneMissCount = 0;
			return true;
		}
		paneMissCount += 1;
		return paneMissCount < PANE_MISS_TOLERANCE;
	};

	while (!finished) {
		for (const line of tail.readNew()) applySessionLine(line);

		if (result.interrupted || result.timedOut || result.stopped) {
			finished = true;
			break;
		}
		if (terminalStopAt && Date.now() - terminalStopAt >= COMPLETION_DRAIN_MS) {
			finished = true;
			break;
		}
		if (!terminalStopAt && !(await isPaneAlive())) {
			paneGone = true;
			finished = true;
			break;
		}
		await new Promise((resolveSleep) => setTimeout(resolveSleep, SESSION_POLL_MS));
	}
	for (const line of tail.readNew()) applySessionLine(line);

	result.finalOutput = getFinalOutput(result.messages);
	if (paneGone && !result.finalOutput) {
		result.exitCode = 1;
		result.error = "Native herdr pane ended before the subagent produced a final answer.";
	}
	if (!result.interrupted && !result.timedOut && !result.stopped) {
		// The workspace is a live "what are my subagents doing" view by default;
		// finished panes auto-close unless the config opts into keeping them.
		if (input.settings.keepPanes) {
			await markPaneDone(result.exitCode === 0 && result.finalOutput ? "✓" : "✗");
		} else if (!paneGone) {
			await closePane();
		}
	}
	return result;
}
