import type { ExternalBackend } from "./external-backends.ts";

export interface ExternalTokenUsage {
	input: number;
	output: number;
	cacheRead: number;
	cacheWrite: number;
}

export type ExternalCliEvent =
	| { type: "assistant_text"; text: string }
	| { type: "tool_start"; id?: string; name: string; input?: unknown }
	| { type: "tool_result"; id?: string; output?: unknown; isError?: boolean }
	| { type: "error"; message: string };

export interface ExternalCliSummary {
	backend: ExternalBackend;
	finalOutput: string;
	sessionId?: string;
	model?: string;
	usage: ExternalTokenUsage;
	turns?: number;
	elapsedMs?: number;
	error?: string;
	terminal: boolean;
}

export interface ExternalCliNormalizationResult {
	events: ExternalCliEvent[];
	summary: ExternalCliSummary;
	diagnostics: string[];
}

const MAX_DIAGNOSTICS = 20;
const MAX_DIAGNOSTIC_LENGTH = 500;
const zeroUsage = (): ExternalTokenUsage => ({ input: 0, output: 0, cacheRead: 0, cacheWrite: 0 });
const record = (value: unknown): Record<string, unknown> | undefined => value !== null && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : undefined;
const string = (value: unknown): string | undefined => typeof value === "string" ? value : undefined;
const number = (value: unknown): number | undefined => typeof value === "number" && Number.isFinite(value) ? value : undefined;

function errorMessage(value: unknown): string | undefined {
	if (typeof value === "string") return value;
	const object = record(value);
	return string(object?.message) ?? string(object?.error);
}

export class ExternalCliEventNormalizer {
	readonly events: ExternalCliEvent[] = [];
	readonly diagnostics: string[] = [];
	readonly summary: ExternalCliSummary;
	readonly backend: ExternalBackend;
	private assistantParts: string[] = [];

	constructor(backend: ExternalBackend) {
		this.backend = backend;
		this.summary = { backend, finalOutput: "", usage: zeroUsage(), terminal: false };
	}

	pushLine(line: string): void {
		if (!line.trim()) return;
		let value: unknown;
		try { value = JSON.parse(line); }
		catch { this.diagnostic(`malformed JSONL: ${line}`); return; }
		const event = record(value);
		if (!event || typeof event.type !== "string") {
			this.diagnostic(`unrecognized JSONL: ${line}`);
			return;
		}
		if (this.backend === "claude-code") this.pushClaude(event, line);
		else this.pushCodex(event, line);
	}

	finish(elapsedMs?: number): ExternalCliNormalizationResult {
		if (elapsedMs !== undefined && this.summary.elapsedMs === undefined) this.summary.elapsedMs = elapsedMs;
		if (!this.summary.finalOutput) this.summary.finalOutput = this.assistantParts.join("");
		return { events: [...this.events], summary: { ...this.summary, usage: { ...this.summary.usage } }, diagnostics: [...this.diagnostics] };
	}

	private diagnostic(message: string): void {
		if (this.diagnostics.length >= MAX_DIAGNOSTICS) return;
		this.diagnostics.push(message.slice(0, MAX_DIAGNOSTIC_LENGTH));
	}

	private addText(text: string): void {
		this.assistantParts.push(text);
		this.events.push({ type: "assistant_text", text });
	}

	private setError(message: string): void {
		this.events.push({ type: "error", message });
		if (!this.summary.error) this.summary.error = message;
	}

	private addUsage(value: unknown, replace: boolean): void {
		const usage = record(value);
		if (!usage) return;
		const next = {
			input: number(usage.input_tokens) ?? 0,
			output: number(usage.output_tokens) ?? 0,
			cacheRead: number(usage.cache_read_input_tokens) ?? number(usage.cached_input_tokens) ?? 0,
			cacheWrite: number(usage.cache_creation_input_tokens) ?? 0,
		};
		if (replace) this.summary.usage = next;
		else {
			this.summary.usage.input += next.input;
			this.summary.usage.output += next.output;
			this.summary.usage.cacheRead += next.cacheRead;
			this.summary.usage.cacheWrite += next.cacheWrite;
		}
	}

	private pushClaude(event: Record<string, unknown>, raw: string): void {
		const type = event.type;
		this.summary.sessionId ??= string(event.session_id);
		if (type === "system" && event.subtype === "init") {
			this.summary.model ??= string(event.model);
			return;
		}
		if (type === "assistant") {
			const message = record(event.message);
			this.summary.model ??= string(message?.model);
			this.addUsage(message?.usage, false);
			const content = Array.isArray(message?.content) ? message.content : [];
			for (const rawBlock of content) {
				const block = record(rawBlock);
				if (block?.type === "text" && typeof block.text === "string") this.addText(block.text);
				else if (block?.type === "tool_use" && typeof block.name === "string") this.events.push({ type: "tool_start", id: string(block.id), name: block.name, input: block.input });
			}
			return;
		}
		if (type === "user") {
			const message = record(event.message);
			const content = Array.isArray(message?.content) ? message.content : [];
			for (const rawBlock of content) {
				const block = record(rawBlock);
				if (block?.type === "tool_result") this.events.push({ type: "tool_result", id: string(block.tool_use_id), output: block.content, isError: block.is_error === true });
			}
			return;
		}
		if (type === "result") {
			if (this.summary.terminal) return;
			this.summary.terminal = true;
			this.summary.sessionId ??= string(event.session_id);
			this.summary.turns = number(event.num_turns);
			this.summary.elapsedMs = number(event.duration_ms);
			this.addUsage(event.usage, true);
			const structured = record(event.structured_output);
			const final = string(event.result) ?? (structured ? JSON.stringify(structured) : undefined);
			if (final !== undefined && event.is_error !== true) this.summary.finalOutput = final;
			const message = event.is_error === true ? final ?? errorMessage(event.error) ?? "Claude Code failed" : errorMessage(event.error);
			if (message) this.setError(message);
			return;
		}
		if (type === "error") { const message = errorMessage(event.error) ?? string(event.message) ?? "Claude Code error"; this.setError(message); return; }
		// Progress and control records do not contain user-facing advice.
		if (type === "rate_limit_event" || type === "stream_event") return;
		this.diagnostic(`unknown Claude event '${String(type)}': ${raw}`);
	}

	private pushCodex(event: Record<string, unknown>, raw: string): void {
		const type = event.type;
		if (type === "thread.started") {
			this.summary.sessionId ??= string(event.thread_id);
			this.summary.model ??= string(event.model);
			return;
		}
		if (type === "turn.started") return;
		if (type === "item.started" || type === "item.completed") {
			const item = record(event.item);
			if (!item) { this.diagnostic(`malformed Codex item: ${raw}`); return; }
			const itemType = string(item.type);
			if (itemType === "agent_message" && type === "item.completed" && typeof item.text === "string") this.addText(item.text);
			else if (type === "item.started" && (itemType === "command_execution" || itemType === "mcp_tool_call" || itemType === "web_search")) {
				this.events.push({ type: "tool_start", id: string(item.id), name: string(item.name) ?? itemType, input: item.command ?? item.arguments ?? item.query });
			} else if (type === "item.completed" && (itemType === "command_execution" || itemType === "mcp_tool_call" || itemType === "web_search")) {
				this.events.push({ type: "tool_result", id: string(item.id), output: item.aggregated_output ?? item.output ?? item.result, isError: item.status === "failed" });
			}
			return;
		}
		if (type === "turn.completed") {
			if (this.summary.terminal) return;
			this.summary.terminal = true;
			this.summary.turns = (this.summary.turns ?? 0) + 1;
			this.addUsage(event.usage, true);
			this.summary.elapsedMs ??= number(event.duration_ms);
			this.summary.finalOutput = this.assistantParts.join("");
			return;
		}
		if (type === "turn.failed" || type === "error") {
			if (this.summary.terminal) return;
			this.summary.terminal = true;
			const message = errorMessage(event.error) ?? string(event.message) ?? "Codex failed";
			this.setError(message);
			return;
		}
		this.diagnostic(`unknown Codex event '${String(type)}': ${raw}`);
	}
}

export function normalizeExternalCliJsonl(backend: ExternalBackend, jsonl: string, elapsedMs?: number): ExternalCliNormalizationResult {
	const normalizer = new ExternalCliEventNormalizer(backend);
	for (const line of jsonl.split(/\r?\n/)) normalizer.pushLine(line);
	return normalizer.finish(elapsedMs);
}

export const normalizeExternalCliEvents = normalizeExternalCliJsonl;
