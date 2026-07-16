import assert from "node:assert/strict";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";
import type { AgentConfig } from "../../src/agents/agents.ts";
import { runSync } from "../../src/runs/foreground/execution.ts";
import { executeAsyncSingle } from "../../src/runs/background/async-execution.ts";
import { createCapabilityGrantStore, type CapabilityRequest } from "../../src/runs/shared/capability-grants.ts";

const fixture = resolve("test/support/fake-external-cli.mjs");
chmodSync(fixture, 0o755);
const agent = (backend: "claude-code" | "codex-cli"): AgentConfig => ({
	name: "external", backend, description: "external", tools: ["read"], model: "model with spaces",
	systemPrompt: "", systemPromptMode: "replace", inheritProjectContext: false, inheritSkills: false,
	source: "project", filePath: "",
});

function capability(backend: "claude-code" | "codex-cli"): CapabilityRequest {
	return { role: "external", roleContent: "integration role", workflow: "integration", workflowContent: "integration workflow", backend, executable: fixture };
}

async function withFixture<T>(backend: "claude-code" | "codex-cli", mode: string, fn: (dir: string, request: CapabilityRequest) => Promise<T>): Promise<T> {
	const dir = mkdtempSync(join(tmpdir(), "pi-external-"));
	const executableKey = backend === "claude-code" ? "PI_SUBAGENTS_CLAUDE_CODE_EXECUTABLE" : "PI_SUBAGENTS_CODEX_CLI_EXECUTABLE";
	const previous = { executable: process.env[executableKey], backend: process.env.FAKE_EXTERNAL_BACKEND, mode: process.env.FAKE_EXTERNAL_MODE, agentDir: process.env.PI_CODING_AGENT_DIR, codexHome: process.env.CODEX_HOME };
	process.env[executableKey] = fixture; process.env.FAKE_EXTERNAL_BACKEND = backend; process.env.FAKE_EXTERNAL_MODE = mode; process.env.PI_CODING_AGENT_DIR = join(dir, "agent-dir");
	process.env.CODEX_HOME = join(dir, "codex-home"); mkdirSync(process.env.CODEX_HOME, { recursive: true }); writeFileSync(join(process.env.CODEX_HOME, "auth.json"), "{}");
	const request = capability(backend);
	createCapabilityGrantStore({ agentDir: process.env.PI_CODING_AGENT_DIR }).grant(request);
	try { return await fn(dir, request); } finally {
		if (previous.executable === undefined) delete process.env[executableKey]; else process.env[executableKey] = previous.executable;
		if (previous.backend === undefined) delete process.env.FAKE_EXTERNAL_BACKEND; else process.env.FAKE_EXTERNAL_BACKEND = previous.backend;
		if (previous.mode === undefined) delete process.env.FAKE_EXTERNAL_MODE; else process.env.FAKE_EXTERNAL_MODE = previous.mode;
		if (previous.agentDir === undefined) delete process.env.PI_CODING_AGENT_DIR; else process.env.PI_CODING_AGENT_DIR = previous.agentDir;
		if (previous.codexHome === undefined) delete process.env.CODEX_HOME; else process.env.CODEX_HOME = previous.codexHome;
		rmSync(dir, { recursive: true, force: true });
	}
}

for (const backend of ["claude-code", "codex-cli"] as const) test(`runSync routes ${backend} through external JSONL execution and preserves spaced args`, async () => {
	await withFixture(backend, "success", async (dir, request) => {
		const argsFile = join(dir, "args.jsonl"); process.env.FAKE_EXTERNAL_ARGS_FILE = argsFile;
		try {
			const result = await runSync(dir, [agent(backend)], "external", "task with spaces", { runId: "run", externalCapabilityRequest: request, artifactsDir: join(dir, "artifacts"), artifactConfig: { enabled: true, includeInput: true, includeOutput: true, includeJsonl: true, includeTranscript: true, includeMetadata: true, cleanupDays: 1 } });
			assert.equal(result.exitCode, 0); assert.equal(result.finalOutput, "external success"); assert.equal(result.usage.input, 3); assert.equal(result.sessionFile, backend === "claude-code" ? "claude-session" : "codex-session");
			const args = JSON.parse(readFileSync(argsFile, "utf8").trim()) as string[];
			assert.ok(args.includes("task with spaces")); assert.ok(args.includes("model with spaces"));
			assert.equal(readFileSync(result.artifactPaths!.outputPath, "utf8"), "external success");
		} finally { delete process.env.FAKE_EXTERNAL_ARGS_FILE; }
	});
});

test("runSync surfaces external failure without fallback", async () => {
	await withFixture("claude-code", "fail", async (dir, request) => {
		const result = await runSync(dir, [agent("claude-code")], "external", "fail", { runId: "fail", externalCapabilityRequest: request });
		assert.equal(result.exitCode, 7); assert.match(result.error ?? "", /exited with code 7|fake failure/);
	});
});

test("runSync cancellation terminates an external process", async () => {
	await withFixture("codex-cli", "wait", async (dir, request) => {
		const controller = new AbortController(); setTimeout(() => controller.abort(), 50);
		const result = await runSync(dir, [agent("codex-cli")], "external", "wait", { runId: "cancel", signal: controller.signal, externalCapabilityRequest: request });
		assert.equal(result.stopped, true); assert.match(result.error ?? "", /cancelled/i);
	});
});

test("detached direct external execution fails closed without an internal workflow capability", async () => {
	await withFixture("claude-code", "success", async (dir) => {
		const id = `external-async-${process.pid}-${Date.now()}`;
		const started = executeAsyncSingle(id, {
			agent: "external", agentConfig: agent("claude-code"), task: "async task with spaces",
			ctx: { cwd: dir, currentSessionId: "test-session", pi: { events: { emit() {} } } as never },
			artifactConfig: { enabled: true, includeInput: true, includeOutput: true, includeJsonl: true, includeTranscript: true, includeMetadata: true, cleanupDays: 1 },
			artifactsDir: join(dir, "artifacts"), shareEnabled: false, maxSubagentDepth: 1,
		});
		assert.equal(started.isError, undefined);
		const statusPath = join(started.details.asyncDir!, "status.json");
		let status: { state?: string; steps?: Array<{ status?: string; output?: string; error?: string }> } | undefined;
		for (let i = 0; i < 100; i++) {
			try { status = JSON.parse(readFileSync(statusPath, "utf8")); } catch {}
			if (status?.state === "complete" || status?.state === "failed") break;
			await new Promise((resolve) => setTimeout(resolve, 50));
		}
		assert.equal(status?.state, "failed", JSON.stringify(status));
		assert.equal(status?.steps?.[0]?.status, "failed");
		assert.match(status?.steps?.[0]?.error ?? "", /requires an exact workflow capability grant/);
		rmSync(started.details.asyncDir!, { recursive: true, force: true });
	});
});
