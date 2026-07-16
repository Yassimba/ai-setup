import * as fs from "node:fs";
import * as path from "node:path";
import type { AgentToolResult } from "@earendil-works/pi-agent-core";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { getAgentDir } from "../../shared/utils.ts";
import { createCapabilityGrantStore, sha256, type CapabilityRequest } from "./capability-grants.ts";
import { resolveExecutable } from "./executable-resolver.ts";
import { canonicalizeExternalContext } from "./external-context.ts";
import { runExternalExecution } from "./external-execution.ts";
import type { HerdrWorkspaceSettings } from "./herdr-workspace.ts";
import { formatRunReceipt } from "./run-receipt.ts";

const ADVISOR_ROLE = `You are Fable, a session-scoped execution advisor. The parent executor owns all execution and decisions. Reconstruct the decisions already made, check for drift from the goal and constraints, and advise the executor. Never use tools. Request exact evidence when evidence is missing. Do not write a user-facing answer.`;
const ADVISOR_WORKFLOW = `For each consultation: identify the crux, test current direction against prior decisions and evidence, then return only the required JSON object. verdict is continue, course_correct, need_evidence, or stop. advice has at most five concise executor actions. evidenceRequests are exact and actionable. recheckAfter states the next concrete checkpoint.`;
export const FABLE_ADVISOR_SCHEMA = {
	type: "object", additionalProperties: false,
	required: ["verdict", "crux", "advice", "evidenceRequests", "risks", "recheckAfter"],
	properties: {
		verdict: { type: "string", enum: ["continue", "course_correct", "need_evidence", "stop"] },
		crux: { type: "string" },
		advice: { type: "array", maxItems: 5, items: { type: "string" } },
		evidenceRequests: { type: "array", items: { type: "string" } },
		risks: { type: "array", items: { type: "string" } },
		recheckAfter: { type: "string" },
	},
} as const;

export type FableAdvice = {
	verdict: "continue" | "course_correct" | "need_evidence" | "stop";
	crux: string;
	advice: string[];
	evidenceRequests: string[];
	risks: string[];
	recheckAfter: string;
};

type AdvisorState = {
	version: 1; parentSessionId: string; enabled: boolean; activationId: string;
	consultations: number; cursor: string[]; claudeSessionId?: string; policyHash: string; guidanceHash?: string;
};

type AdvisorParams = { action?: string; message?: string; task?: string };
type BranchEntry = Record<string, unknown>;
type AdvisorDetails = { mode: "management"; results: []; advisor: Record<string, unknown> };

function management(text: string, advisor: Record<string, unknown>, isError = false): AgentToolResult<AdvisorDetails> {
	return { content: [{ type: "text", text }], ...(isError ? { isError: true } : {}), details: { mode: "management", results: [], advisor } };
}

function executable(): string {
	return resolveExecutable(process.env.PI_SUBAGENTS_CLAUDE_CODE_EXECUTABLE ?? "claude").path;
}

function skillContent(): string {
	try { return fs.readFileSync(new URL("../../../skills/fable-advisor/SKILL.md", import.meta.url), "utf8"); }
	catch { return ADVISOR_WORKFLOW; }
}

export function fableCapabilityRequest(): CapabilityRequest {
	return { role: "fable-advisor", roleContent: ADVISOR_ROLE, workflow: "fable-advisor", workflowContent: skillContent(), backend: "claude-code", executable: executable() };
}

function sessionIdentity(ctx: ExtensionContext): { id: string; dir: string } | undefined {
	const file = ctx.sessionManager.getSessionFile();
	if (!file) return undefined;
	const id = ctx.sessionManager.getSessionId() || file;
	const base = path.basename(file, path.extname(file));
	return { id, dir: path.join(path.dirname(file), base, "subagent-artifacts", "advisor") };
}

function readState(file: string): AdvisorState | undefined {
	try {
		const state = JSON.parse(fs.readFileSync(file, "utf8")) as AdvisorState;
		return state?.version === 1 && Array.isArray(state.cursor) ? state : undefined;
	} catch { return undefined; }
}

function writeState(file: string, state: AdvisorState): void {
	fs.mkdirSync(path.dirname(file), { recursive: true });
	const temp = `${file}.${process.pid}.tmp`;
	fs.writeFileSync(temp, `${JSON.stringify(state, null, 2)}\n`, "utf8");
	fs.renameSync(temp, file);
}

function entrySignature(entry: BranchEntry): string {
	return sha256(JSON.stringify(entry));
}

function textContent(content: unknown): string {
	if (typeof content === "string") return content;
	if (!Array.isArray(content)) return "";
	return content.filter((part): part is { type: string; text: string } => Boolean(part && typeof part === "object" && (part as { type?: unknown }).type === "text" && typeof (part as { text?: unknown }).text === "string")).map((part) => part.text).join("\n");
}

function transcript(entries: BranchEntry[]): Array<{ role: "user" | "assistant"; text: string }> {
	const output: Array<{ role: "user" | "assistant"; text: string }> = [];
	for (const entry of entries) {
		if (entry.type !== "message" || !entry.message || typeof entry.message !== "object") continue;
		const message = entry.message as Record<string, unknown>;
		if (message.role !== "user" && message.role !== "assistant") continue;
		const text = textContent(message.content).trim();
		if (text) output.push({ role: message.role, text });
	}
	return output;
}

function projectGuidance(cwd: string): Array<{ file: string; text: string }> {
	const result: Array<{ file: string; text: string }> = [];
	for (const name of ["AGENTS.md", "CLAUDE.md"]) {
		const file = path.join(cwd, name);
		try { result.push({ file: name, text: fs.readFileSync(file, "utf8") }); } catch {}
	}
	return result;
}

export function buildFableContext(cwd: string, entries: BranchEntry[], question: string, mode: "advisor-seed" | "advisor-delta", guidance = projectGuidance(cwd)) {
	const sections = [
		{ kind: "goal-question", priority: 10, value: question },
		...(mode === "advisor-seed" ? [{ kind: "constraints-project-guidance", priority: 20, value: guidance }] : []),
		{ kind: "decision-transcript", priority: 30, value: transcript(entries) },
		{ kind: "selected-evidence-current-state", priority: 40, value: { cwd: "." } },
	];
	return canonicalizeExternalContext({ workspace: cwd, sections }, { mode });
}

function validateAdvice(value: unknown): value is FableAdvice {
	if (!value || typeof value !== "object" || Array.isArray(value)) return false;
	const v = value as Record<string, unknown>;
	const keys = Object.keys(v).sort().join(",");
	if (keys !== ["advice", "crux", "evidenceRequests", "recheckAfter", "risks", "verdict"].sort().join(",")) return false;
	if (!["continue", "course_correct", "need_evidence", "stop"].includes(String(v.verdict))) return false;
	if (typeof v.crux !== "string" || typeof v.recheckAfter !== "string") return false;
	return [v.advice, v.evidenceRequests, v.risks].every((items) => Array.isArray(items) && items.every((item) => typeof item === "string")) && (v.advice as unknown[]).length <= 5;
}

function policyHash(): string { return sha256(`${ADVISOR_ROLE}\n${ADVISOR_WORKFLOW}\n${JSON.stringify(FABLE_ADVISOR_SCHEMA)}`); }

export async function handleFableAdvisorAction(params: AdvisorParams, signal: AbortSignal, ctx: ExtensionContext, herdrWorkspace?: HerdrWorkspaceSettings): Promise<AgentToolResult<AdvisorDetails>> {
	const action = params.action ?? "";
	const session = sessionIdentity(ctx);
	if (!session) return management("Fable advisor requires a persisted parent Pi session.", { action }, true);
	const file = path.join(session.dir, "state.json");
	const grants = createCapabilityGrantStore({ agentDir: getAgentDir() });
	const request = fableCapabilityRequest();
	if (action === "advisor.activate") {
		let verification = grants.verify(request);
		if (!verification.allowed && ctx.hasUI) {
			const approved = await ctx.ui.confirm("Activate Fable advisor?", `Grant exact capability to bundled role 'fable-advisor', workflow 'fable-advisor', and executable '${request.executable}'. The external advisor is tool-free and session-scoped.`);
			if (approved) { grants.grant(request); verification = grants.verify(request); }
		}
		if (!verification.allowed) return management(`Fable advisor activation denied: ${verification.reason}. Headless activation requires an exact pregrant.`, { action, grant: verification.reason }, true);
		const state: AdvisorState = { version: 1, parentSessionId: session.id, enabled: true, activationId: sha256(`${session.id}\n${policyHash()}`).slice(0, 16), consultations: 0, cursor: [], policyHash: policyHash() };
		writeState(file, state);
		return management("Fable advisor activated for this parent session.", { action, enabled: true, consultations: 0, maxConsultations: maxConsultations() });
	}
	const state = readState(file);
	if (!state || state.parentSessionId !== session.id) return management("Fable advisor is not activated for this parent session.", { action, enabled: false }, true);
	if (action === "advisor.status") return management(`Fable advisor is ${state.enabled ? "active" : "disabled"} (${state.consultations}/${maxConsultations()} consultations).`, { action, enabled: state.enabled, consultations: state.consultations, maxConsultations: maxConsultations(), resumable: Boolean(state.claudeSessionId) });
	if (action === "advisor.disable") { state.enabled = false; state.claudeSessionId = undefined; state.cursor = []; writeState(file, state); return management("Fable advisor disabled for this parent session.", { action, enabled: false }); }
	if (action === "advisor.reset") { try { fs.rmSync(file, { force: true }); } catch {} return management("Fable advisor mapping reset for this parent session.", { action, enabled: false }); }
	if (action !== "advisor.ask") return management(`Unknown advisor action: ${action}.`, { action }, true);
	if (!state.enabled) return management("Fable advisor is disabled for this parent session.", { action, enabled: false }, true);
	const verification = grants.verify(request);
	if (!verification.allowed) return management(`Fable advisor grant check failed: ${verification.reason}.`, { action, grant: verification.reason }, true);
	if (state.consultations >= maxConsultations()) return management(`Fable advisor consultation limit reached (${maxConsultations()}).`, { action, consultations: state.consultations }, true);
	const question = (params.message ?? params.task ?? "Review current execution direction.").trim();
	const branch = ctx.sessionManager.getBranch() as BranchEntry[];
	const signatures = branch.map(entrySignature);
	const append = state.cursor.length <= signatures.length && state.cursor.every((item, index) => signatures[index] === item);
	const policyChanged = state.policyHash !== policyHash();
	const guidance = projectGuidance(ctx.cwd);
	const guidanceHash = sha256(JSON.stringify(guidance));
	const guidanceChanged = state.guidanceHash !== undefined && state.guidanceHash !== guidanceHash;
	const compacted = branch.some((entry) => entry.type === "compaction");
	const resume = Boolean(state.claudeSessionId && append && !policyChanged && !guidanceChanged && !compacted);
	const entries = resume ? branch.slice(state.cursor.length) : branch;
	const context = buildFableContext(ctx.cwd, entries, question, resume ? "advisor-delta" : "advisor-seed", guidance);
	const basePrompt = `${ADVISOR_ROLE}\n\n${ADVISOR_WORKFLOW}\n\n${resume ? "DELTA SINCE LAST CONSULTATION" : "SESSION SEED"}:\n${context.text}`;
	const run = (resumeId?: string) => runExternalExecution({
		agent: { name: "fable-advisor", backend: "claude-code", description: "Fable advisor", tools: [], model: "fable", thinking: "high", systemPrompt: "", systemPromptMode: "replace", inheritProjectContext: false, inheritSkills: false, source: "builtin", filePath: "" },
		task: basePrompt, cwd: ctx.cwd, model: "fable", thinking: "high", tools: [], sessionEnabled: true,
		resumeSessionId: resumeId, jsonSchema: FABLE_ADVISOR_SCHEMA, signal, artifactsDir: session.dir,
		runId: `advisor-${state.activationId}-${state.consultations + 1}`, source: "foreground", capabilityRequest: request, herdrWorkspace,
	});
	let execution = await run(resume ? state.claudeSessionId : undefined);
	let retriedFresh = false;
	if (resume && !execution.stopped && execution.error) { retriedFresh = true; execution = await run(); }
	if (execution.stopped || signal.aborted) {
		state.claudeSessionId = undefined; state.cursor = []; writeState(file, state);
		return management("Fable advisor consultation cancelled; resumable mapping discarded.", { action, cancelled: true, seeded: !resume, resumed: resume }, true);
	}
	const error = execution.error;
	if (error) { state.claudeSessionId = undefined; state.cursor = []; writeState(file, state); return management(`Fable advisor failed: ${error}`, { action, retriedFresh }, true); }
	let advice: unknown;
	try { advice = JSON.parse(execution.finalOutput ?? ""); } catch {}
	if (!validateAdvice(advice)) { state.claudeSessionId = undefined; state.cursor = []; writeState(file, state); return management("Fable advisor returned output that does not match the required schema.", { action, schemaValid: false }, true); }
	state.consultations++;
	state.cursor = signatures;
	state.policyHash = policyHash();
	state.guidanceHash = guidanceHash;
	state.claudeSessionId = execution.sessionFile;
	writeState(file, state);
	const usage = execution.usage;
	const receipt = { seeded: !resume || retriedFresh, resumed: resume && !retriedFresh, reseeded: !append || policyChanged || guidanceChanged || compacted || retriedFresh, contextBytes: context.bytes, contextHash: context.hash, cacheRead: usage.cacheRead, cacheWrite: usage.cacheWrite, inputTokens: usage.input, outputTokens: usage.output };
	const formattedReceipt = formatRunReceipt({
		kind: "advisor", backend: "claude-code", model: execution.model ?? "fable", effort: "high",
		durationMs: execution.progressSummary?.durationMs, inputTokens: usage.input, outputTokens: usage.output,
		cacheReadTokens: usage.cacheRead, cacheWriteTokens: usage.cacheWrite, retries: retriedFresh ? 1 : 0,
		resumed: receipt.resumed, contextHash: context.hash,
		executionMode: execution.externalExecutionMode,
		workspaceId: execution.externalWorkspaceId,
		paneId: execution.externalPaneId,
	});
	return management(`${JSON.stringify({ advice, metadata: { consultation: state.consultations, maxConsultations: maxConsultations(), ...receipt } })}\n\n${formattedReceipt}`, { action, advice, receipt: { ...receipt, formatted: formattedReceipt }, claudeSessionId: state.claudeSessionId ?? null });
}

function maxConsultations(): number {
	const configured = Number(process.env.PI_SUBAGENTS_ADVISOR_MAX_CONSULTATIONS ?? 3);
	return Number.isInteger(configured) && configured > 0 ? configured : 3;
}
