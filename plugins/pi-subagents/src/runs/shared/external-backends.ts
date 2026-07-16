export type AgentBackend = "pi" | "claude-code" | "codex-cli";
export type ExternalBackend = Exclude<AgentBackend, "pi">;
export type ExternalTool = "read" | "grep" | "find" | "web_search" | "fetch_content";

export interface CommandSpec {
	command: string;
	args: string[];
	env?: Record<string, string>;
}

const EXTERNAL_TOOLS = new Set<ExternalTool>(["read", "grep", "find", "web_search", "fetch_content"]);
const CLAUDE_TOOLS: Record<ExternalTool, string> = {
	read: "Read",
	grep: "Grep",
	find: "Glob",
	web_search: "WebSearch",
	fetch_content: "WebFetch",
};

export function selectBackend(backend: AgentBackend | undefined): AgentBackend {
	return backend ?? "pi";
}

export const resolveBackend = selectBackend;

export function normalizeExternalTools(tools: readonly string[]): ExternalTool[] {
	const normalized: ExternalTool[] = [];
	const seen = new Set<ExternalTool>();
	for (const rawTool of tools) {
		const tool = rawTool.trim();
		if (!EXTERNAL_TOOLS.has(tool as ExternalTool)) throw new Error(`Unknown external tool '${tool}'.`);
		const externalTool = tool as ExternalTool;
		if (!seen.has(externalTool)) {
			seen.add(externalTool);
			normalized.push(externalTool);
		}
	}
	return normalized;
}

export function mapExternalTools(backend: ExternalBackend, tools: readonly string[]): string[] {
	const normalized = normalizeExternalTools(tools);
	return backend === "claude-code" ? normalized.map((tool) => CLAUDE_TOOLS[tool]) : normalized;
}

export interface ClaudeCommandInput {
	executable: string;
	prompt: string;
	tools: readonly string[];
	model?: string;
	effort?: string;
	permissionMode: "plan" | "dontAsk";
	sessionMode: "ephemeral" | "resumable";
	resumeSessionId?: string;
	jsonSchema?: Record<string, unknown>;
}

export function buildClaudeCommand(input: ClaudeCommandInput): CommandSpec {
	const args = [
		"-p",
		input.prompt,
		"--output-format",
		"stream-json",
		"--verbose",
		"--safe-mode",
		"--prompt-suggestions",
		"false",
		"--permission-mode",
		input.permissionMode,
		"--strict-mcp-config",
		"--tools",
		mapExternalTools("claude-code", input.tools).join(","),
	];
	if (input.sessionMode === "ephemeral") args.push("--no-session-persistence");
	if (input.resumeSessionId) args.push("--resume", input.resumeSessionId);
	if (input.jsonSchema) args.push("--json-schema", JSON.stringify(input.jsonSchema));
	if (input.model) args.push("--model", input.model);
	if (input.effort) args.push("--effort", input.effort);
	return { command: input.executable, args };
}

export const buildClaudeCommandSpec = buildClaudeCommand;

export interface CodexCommandInput {
	executable: string;
	prompt: string;
	tools: readonly string[];
	model?: string;
	reasoningEffort?: string;
	ephemeral: boolean;
	resumeSessionId?: string;
}

export function buildCodexCommand(input: CodexCommandInput): CommandSpec {
	const tools = normalizeExternalTools(input.tools);
	// Approval policy is a global Codex option and must precede the `exec` subcommand.
	const args = ["--ask-for-approval", "never", "exec"];
	if (input.resumeSessionId) args.push("resume", input.resumeSessionId);
	args.push(
		"--json",
		"--sandbox",
		"read-only",
		"--ignore-user-config",
		"--ignore-rules",
		"-c",
		"project_doc_max_bytes=0",
	);
	if (input.ephemeral) args.push("--ephemeral");
	if (tools.some((tool) => tool === "web_search" || tool === "fetch_content")) {
		args.push("-c", 'web_search="live"');
	}
	if (input.model) args.push("--model", input.model);
	if (input.reasoningEffort) args.push("-c", `model_reasoning_effort=${JSON.stringify(input.reasoningEffort)}`);
	args.push(input.prompt);
	return { command: input.executable, args };
}

export const buildCodexCommandSpec = buildCodexCommand;
