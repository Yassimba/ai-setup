import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import type { AgentConfig } from "../../agents/agents.ts";
import { createChildTranscriptWriter } from "../../shared/child-transcript.ts";
import { ensureArtifactsDir, getArtifactPaths, writeArtifact, writeMetadata } from "../../shared/artifacts.ts";
import type { AgentProgress, ArtifactConfig, ArtifactPaths, SingleResult, Usage } from "../../shared/types.ts";
import { buildClaudeCommand, buildCodexCommand, selectBackend, type ExternalBackend } from "./external-backends.ts";
import { normalizeExternalCliJsonl } from "./external-cli-events.ts";
import { runExternalProcess } from "./external-process.ts";
import { runHerdrExternalProcess } from "./herdr-external-process.ts";
import type { HerdrWorkspaceSettings } from "./herdr-workspace.ts";
import { createCapabilityGrantStore, resolveCanonicalExecutable, type CapabilityRequest } from "./capability-grants.ts";
import { getAgentDir } from "../../shared/utils.ts";

export interface ExternalExecutionInput {
	agent: AgentConfig;
	task: string;
	cwd: string;
	model?: string;
	thinking?: string | false;
	tools?: string[];
	sessionFile?: string;
	sessionEnabled?: boolean;
	resumeSessionId?: string;
	jsonSchema?: Record<string, unknown>;
	timeoutMs?: number;
	signal?: AbortSignal;
	artifactsDir?: string;
	artifactConfig?: Partial<ArtifactConfig>;
	runId: string;
	index?: number;
	source: "foreground" | "async";
	onProgress?: (progress: AgentProgress) => void;
	/** Internal workflow authorization. Backend declaration alone never grants execution. */
	capabilityRequest?: CapabilityRequest;
	/** Route the safe external command through a visible Herdr pane when configured. */
	herdrWorkspace?: HerdrWorkspaceSettings;
}

function usage(input: { input: number; output: number; cacheRead: number; cacheWrite: number }, turns = 0): Usage {
	return { ...input, cost: 0, turns };
}

function createIsolatedCodexHome(): string {
	const sourceHome = process.env.CODEX_HOME || path.join(os.homedir(), ".codex");
	const sourceAuth = path.join(sourceHome, "auth.json");
	const targetHome = fs.mkdtempSync(path.join(os.tmpdir(), "pi-subagents-codex-"));
	try {
		fs.copyFileSync(sourceAuth, path.join(targetHome, "auth.json"), fs.constants.COPYFILE_EXCL);
		fs.chmodSync(targetHome, 0o700);
		fs.chmodSync(path.join(targetHome, "auth.json"), 0o600);
		return targetHome;
	} catch (error) {
		fs.rmSync(targetHome, { recursive: true, force: true });
		throw error;
	}
}

/** Single callable boundary for all external CLI execution. Runtime grants can precede this call. */
export async function runExternalExecution(input: ExternalExecutionInput): Promise<SingleResult> {
	const selected = selectBackend(input.agent.backend);
	if (selected === "pi") throw new Error("runExternalExecution only accepts an external agent backend.");
	const backend = selected as ExternalBackend;
	const capability = input.capabilityRequest;
	const denied = (error: string): SingleResult => ({
		agent: input.agent.name, task: input.task, exitCode: 1, error,
		usage: usage({ input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }),
	});
	if (!capability || capability.role !== input.agent.name || capability.backend !== backend) {
		return denied(`External backend '${backend}' for role '${input.agent.name}' requires an exact workflow capability grant.`);
	}
	const startedAt = Date.now();
	const progress: AgentProgress = {
		index: input.index ?? 0, agent: input.agent.name, status: "running", task: input.task,
		recentTools: [], recentOutput: [], toolCount: 0, tokens: 0, durationMs: 0,
	};
	input.onProgress?.({ ...progress, recentTools: [], recentOutput: [] });

	let artifactPaths: ArtifactPaths | undefined;
	let transcript: ReturnType<typeof createChildTranscriptWriter> | undefined;
	if (input.artifactsDir && input.artifactConfig?.enabled !== false) {
		ensureArtifactsDir(input.artifactsDir);
		artifactPaths = getArtifactPaths(input.artifactsDir, input.runId, input.agent.name, input.index);
		if (input.artifactConfig?.includeInput !== false) writeArtifact(artifactPaths.inputPath, `# Task for ${input.agent.name}\n\n${input.task}`);
		if (input.artifactConfig?.includeTranscript !== false) {
			transcript = createChildTranscriptWriter({ transcriptPath: artifactPaths.transcriptPath, source: input.source, runId: input.runId, agent: input.agent.name, childIndex: input.index, cwd: input.cwd });
			transcript.writeInitialUserMessage(input.task);
		}
	}

	let isolatedCodexHome: string | undefined;
	let spec;
	try {
		if (backend === "codex-cli" && !(input.sessionEnabled || input.sessionFile)) isolatedCodexHome = createIsolatedCodexHome();
		// Execute the exact canonical path bound into the verified grant. Never let
		// the local process or a Herdr pane resolve a different binary from PATH.
		const canonicalExecutable = resolveCanonicalExecutable(capability.executable);
		spec = backend === "claude-code"
			? buildClaudeCommand({ executable: canonicalExecutable, prompt: input.task, tools: input.tools ?? input.agent.tools ?? [], model: input.model ?? input.agent.model, effort: input.thinking || undefined, permissionMode: "dontAsk", sessionMode: input.sessionEnabled || input.sessionFile || input.resumeSessionId ? "resumable" : "ephemeral", resumeSessionId: input.resumeSessionId, jsonSchema: input.jsonSchema })
			: buildCodexCommand({ executable: canonicalExecutable, prompt: input.task, tools: input.tools ?? input.agent.tools ?? [], model: input.model ?? input.agent.model, reasoningEffort: input.thinking || undefined, ephemeral: !(input.sessionEnabled || input.sessionFile || input.resumeSessionId), resumeSessionId: input.resumeSessionId ?? input.sessionFile });
		if (isolatedCodexHome) spec.env = { ...spec.env, CODEX_HOME: isolatedCodexHome };
	} catch (error) {
		if (isolatedCodexHome) fs.rmSync(isolatedCodexHome, { recursive: true, force: true });
		return denied(`Unable to create isolated ${backend} runtime: ${error instanceof Error ? error.message : "unknown error"}`);
	}
	// This is intentionally the final operation before handing the command to a
	// process runner. Preparing artifacts or command specs must never consume a grant.
	const verification = createCapabilityGrantStore({ agentDir: getAgentDir() }).verify(capability);
	if (!verification.allowed) {
		if (isolatedCodexHome) fs.rmSync(isolatedCodexHome, { recursive: true, force: true });
		return denied(`External capability grant denied immediately before execution: ${verification.reason}.`);
	}
	let processResult;
	let executionMode: "herdr" | "local" | "local-fallback" = "local";
	try {
		if (input.herdrWorkspace) {
			const visible = await runHerdrExternalProcess(spec, { cwd: input.cwd, timeoutMs: input.timeoutMs, signal: input.signal, settings: input.herdrWorkspace, label: `${input.agent.name}-${input.runId}-${input.index ?? 0}` });
			if (visible) { processResult = visible; executionMode = visible.executionMode; }
			else { processResult = await runExternalProcess(spec, { cwd: input.cwd, timeoutMs: input.timeoutMs, signal: input.signal }); executionMode = "local-fallback"; }
		} else processResult = await runExternalProcess(spec, { cwd: input.cwd, timeoutMs: input.timeoutMs, signal: input.signal });
	} finally {
		if (isolatedCodexHome) fs.rmSync(isolatedCodexHome, { recursive: true, force: true });
	}
	const normalized = normalizeExternalCliJsonl(backend, processResult.stdout, processResult.elapsedMs);
	for (const line of processResult.stdout.split(/\r?\n/)) if (line) transcript?.writeStdoutLine(line);
	if (processResult.stderr) transcript?.writeStderrText(processResult.stderr);
	for (const event of normalized.events) {
		if (event.type === "assistant_text") progress.recentOutput.push(event.text);
		else if (event.type === "tool_start") {
			progress.toolCount++;
			progress.currentTool = event.name;
			progress.currentToolArgs = event.input === undefined ? undefined : JSON.stringify(event.input);
			progress.recentTools.push({ tool: event.name, args: progress.currentToolArgs ?? "", endMs: 0 });
		} else if (event.type === "tool_result") {
			progress.currentTool = undefined;
			const latest = progress.recentTools.at(-1); if (latest) latest.endMs = Date.now();
		}
	}
	progress.tokens = normalized.summary.usage.input + normalized.summary.usage.output;
	progress.turnCount = normalized.summary.turns;
	progress.durationMs = processResult.elapsedMs;
	const error = processResult.error ?? normalized.summary.error ?? (!normalized.summary.terminal ? `External ${backend} did not produce a terminal JSONL event.` : undefined);
	await processResult.finishPane?.(error ? "failure" : "success");
	progress.status = error ? "failed" : "completed";
	progress.error = error;
	input.onProgress?.({ ...progress, recentTools: progress.recentTools.map((x) => ({ ...x })), recentOutput: [...progress.recentOutput] });
	const result: SingleResult = {
		agent: input.agent.name, task: input.task, exitCode: error ? (processResult.exitCode ?? 1) : 0,
		usage: usage(normalized.summary.usage, normalized.summary.turns), model: normalized.summary.model ?? input.model ?? input.agent.model,
		externalExecutionMode: executionMode,
		externalWorkspaceId: processResult.workspaceId,
		externalPaneId: processResult.paneId,
		error, timedOut: processResult.timedOut || undefined, stopped: processResult.cancelled || undefined,
		finalOutput: normalized.summary.finalOutput, progress, progressSummary: { toolCount: progress.toolCount, tokens: progress.tokens, durationMs: progress.durationMs },
		artifactPaths, transcriptPath: transcript ? artifactPaths?.transcriptPath : undefined, transcriptError: transcript?.getError(),
		...(normalized.summary.sessionId ? { sessionFile: normalized.summary.sessionId } : {}),
	};
	if (artifactPaths && input.artifactConfig?.enabled !== false) {
		if (input.artifactConfig?.includeJsonl !== false) writeArtifact(artifactPaths.jsonlPath, processResult.stdout);
		if (input.artifactConfig?.includeOutput !== false) writeArtifact(artifactPaths.outputPath, result.finalOutput ?? "");
		if (input.artifactConfig?.includeMetadata !== false) writeMetadata(artifactPaths.metadataPath, { runId: input.runId, agent: input.agent.name, backend, task: input.task, exitCode: result.exitCode, usage: result.usage, model: result.model, sessionId: normalized.summary.sessionId, durationMs: Date.now() - startedAt, toolCount: progress.toolCount, error, diagnostics: normalized.diagnostics, executionMode, workspaceId: processResult.workspaceId, paneId: processResult.paneId, timestamp: Date.now() });
	}
	return result;
}
