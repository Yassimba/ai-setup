import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import test from "node:test";
import type { AgentConfig } from "../../src/agents/agents.ts";
import { createCapabilityGrantStore, type CapabilityRequest } from "../../src/runs/shared/capability-grants.ts";
import { runExternalExecution } from "../../src/runs/shared/external-execution.ts";
import { HerdrWorkspaceManager, resetHerdrWorkspaceManagerForTests } from "../../src/runs/shared/herdr-workspace.ts";
import { runPiNativeHerdr } from "../../src/runs/background/herdr-native.ts";

const cli = path.resolve("test/support/fake-external-cli.mjs");
const herdr = path.resolve("test/support/fake-herdr.mjs");
const nativePi = path.resolve("test/support/fake-pi-native.mjs");
fs.chmodSync(cli, 0o755); fs.chmodSync(herdr, 0o755); fs.chmodSync(nativePi, 0o755);
const agent: AgentConfig = { name: "visible", backend: "claude-code", description: "visible", tools: [], model: "fable", systemPrompt: "", systemPromptMode: "replace", inheritProjectContext: false, inheritSkills: false, source: "builtin", filePath: "" };
const request: CapabilityRequest = { role: "visible", roleContent: "role", workflow: "test", workflowContent: "workflow", backend: "claude-code", executable: cli };

async function fixture<T>(mode: string, fn: (root: string, state: string) => Promise<T>): Promise<T> {
	const root = fs.mkdtempSync(path.join(os.tmpdir(), "herdr-external-test-")), state = path.join(root, "herdr.json");
	const previous = { agent: process.env.PI_CODING_AGENT_DIR, cli: process.env.PI_SUBAGENTS_CLAUDE_CODE_EXECUTABLE, herdr: process.env.PI_SUBAGENTS_HERDR_EXECUTABLE, state: process.env.FAKE_HERDR_STATE, mode: process.env.FAKE_EXTERNAL_MODE, backend: process.env.FAKE_EXTERNAL_BACKEND };
	process.env.PI_CODING_AGENT_DIR = path.join(root, "agent"); process.env.PI_SUBAGENTS_CLAUDE_CODE_EXECUTABLE = cli; process.env.PI_SUBAGENTS_HERDR_EXECUTABLE = herdr; process.env.FAKE_HERDR_STATE = state; process.env.FAKE_EXTERNAL_MODE = mode; process.env.FAKE_EXTERNAL_BACKEND = "claude-code";
	createCapabilityGrantStore({ agentDir: process.env.PI_CODING_AGENT_DIR }).grant(request); resetHerdrWorkspaceManagerForTests();
	try { return await fn(root, state); } finally { try { const saved = JSON.parse(fs.readFileSync(state, "utf8")); for (const agent of saved.agents ?? []) if (agent.child_pid) { try { process.kill(-agent.child_pid, "SIGTERM"); } catch {} } } catch {} for (const [key, value] of Object.entries(previous)) { const name = ({ agent: "PI_CODING_AGENT_DIR", cli: "PI_SUBAGENTS_CLAUDE_CODE_EXECUTABLE", herdr: "PI_SUBAGENTS_HERDR_EXECUTABLE", state: "FAKE_HERDR_STATE", mode: "FAKE_EXTERNAL_MODE", backend: "FAKE_EXTERNAL_BACKEND" } as const)[key as keyof typeof previous]; if (value === undefined) delete process.env[name]; else process.env[name] = value; } fs.rmSync(root, { recursive: true, force: true }); resetHerdrWorkspaceManagerForTests(); }
}

function run(root: string, extra: Partial<Parameters<typeof runExternalExecution>[0]> = {}) { return runExternalExecution({ agent, task: "PRIVATE PROMPT value", cwd: root, runId: "run", source: "foreground", capabilityRequest: request, herdrWorkspace: { workspaceLabel: "subagents", keepPanes: false }, ...extra }); }

test("the extracted workspace manager preserves native Pi Herdr execution", async () => fixture("success", async (root) => {
	const previous = process.env.PI_SUBAGENT_PI_BINARY; process.env.PI_SUBAGENT_PI_BINARY = nativePi;
	try {
		const sessionFile = path.join(root, "native-session.jsonl");
		const result = await runPiNativeHerdr({ args: ["--session", sessionFile], cwd: root, sessionFile, agentName: "native-smoke", runId: "native", stepIndex: 0, settings: { workspaceLabel: "subagents", keepPanes: false } });
		assert.equal(result.exitCode, 0); assert.equal(result.finalOutput, "native herdr works"); assert.equal(result.usage.input, 2); assert.equal(result.model, "fake/native");
	} finally { if (previous === undefined) delete process.env.PI_SUBAGENT_PI_BINARY; else process.env.PI_SUBAGENT_PI_BINARY = previous; }
}));

test("Herdr external runner uses a private envelope, hides prompts from argv, and returns local-equivalent output", async () => fixture("success", async (root, stateFile) => {
	const result = await run(root); assert.equal(result.exitCode, 0); assert.equal(result.finalOutput, "external success"); assert.equal(result.externalExecutionMode, "herdr");
	assert.equal(result.externalWorkspaceId, "workspace-1"); assert.equal(result.externalPaneId, "pane-1");
	const state = JSON.parse(fs.readFileSync(stateFile, "utf8")); assert.equal(state.envelopeMode, 0o600); assert.match(JSON.stringify(state.envelope), /PRIVATE PROMPT value/);
	assert.equal(state.envelope.spec.command, cli, "the pane executes the canonical path bound by the grant");
	const start = state.calls.find((call: string[]) => call[0] === "agent" && call[1] === "start"); assert.ok(start.includes("--no-focus")); assert.doesNotMatch(JSON.stringify(start), /PRIVATE PROMPT value/);
	assert.ok(state.calls.some((call: string[]) => call[0] === "pane" && call[1] === "close" && call[2] === "pane-1"));
}));

test("a symlinked grant request still executes the canonical real binary", async () => fixture("success", async (root, stateFile) => {
	const link = path.join(root, "claude-link"); fs.symlinkSync(cli, link);
	const linkedRequest = { ...request, executable: link };
	createCapabilityGrantStore({ agentDir: process.env.PI_CODING_AGENT_DIR! }).grant(linkedRequest);
	const result = await runExternalExecution({ agent, task: "private", cwd: root, runId: "symlink", source: "foreground", capabilityRequest: linkedRequest, herdrWorkspace: { workspaceLabel: "subagents", keepPanes: false } });
	assert.equal(result.exitCode, 0);
	const state = JSON.parse(fs.readFileSync(stateFile, "utf8"));
	assert.equal(state.envelope.spec.command, fs.realpathSync(cli));
}));

test("Herdr external timeout closes only its pane", async () => fixture("wait", async (root, stateFile) => {
	const result = await run(root, { timeoutMs: 80 }); assert.equal(result.timedOut, true);
	const state = JSON.parse(fs.readFileSync(stateFile, "utf8")); const closes = state.calls.filter((call: string[]) => call[0] === "pane" && call[1] === "close"); assert.ok(closes.some((call: string[]) => call[2] === "pane-1"));
}));

test("Herdr external cancellation closes only its pane", async () => fixture("wait", async (root, stateFile) => {
	const controller = new AbortController(); setTimeout(() => controller.abort(), 80);
	const result = await run(root, { signal: controller.signal }); assert.equal(result.stopped, true);
	const state = JSON.parse(fs.readFileSync(stateFile, "utf8")); assert.ok(state.calls.some((call: string[]) => call[0] === "pane" && call[1] === "close" && call[2] === "pane-1"));
}));

test("the shared manager reuses one workspace across sequential visible runs", async () => fixture("success", async (root, stateFile) => {
	await run(root); await run(root, { runId: "second" }); const state = JSON.parse(fs.readFileSync(stateFile, "utf8"));
	assert.equal(state.calls.filter((call: string[]) => call[0] === "workspace" && call[1] === "create").length, 1);
	assert.equal(state.calls.filter((call: string[]) => call[0] === "agent" && call[1] === "start").length, 2);
}));

test("an existing populated workspace reconstructs grid anchors for new panes", async () => fixture("success", async (root, stateFile) => {
	fs.writeFileSync(stateFile, JSON.stringify({ workspace: { label: "subagents", workspace_id: "workspace-existing" }, workspace_seq: 1, panes: [{ pane_id: "existing-top-1" }, { pane_id: "existing-top-2" }], agents: [{ name: "a", pane_id: "existing-top-1", workspace_id: "workspace-existing" }, { name: "b", pane_id: "existing-top-2", workspace_id: "workspace-existing" }], calls: [] }));
	resetHerdrWorkspaceManagerForTests();
	const manager = new HerdrWorkspaceManager({ workspaceLabel: "subagents", keepPanes: false });
	const pane = await manager.startPane({ label: "third", cwd: root, command: nativePi, args: ["--session", path.join(root, "grid-session.jsonl")] });
	assert.ok(pane);
	const state = JSON.parse(fs.readFileSync(stateFile, "utf8"));
	const placement = state.calls.find((call: string[]) => call[0] === "pane" && call[1] === "move" && call.includes("--target-pane"));
	assert.equal(placement?.[placement.indexOf("--target-pane") + 1], "existing-top-1");
	await manager.interruptPane(pane!);
}));

test("an auto-closed cached workspace is recreated instead of forcing local fallback", async () => fixture("success", async (root, stateFile) => {
	await run(root);
	const state = JSON.parse(fs.readFileSync(stateFile, "utf8")); state.workspace = null; fs.writeFileSync(stateFile, JSON.stringify(state));
	const second = await run(root, { runId: "after-auto-close" });
	assert.equal(second.externalExecutionMode, "herdr");
	const finalState = JSON.parse(fs.readFileSync(stateFile, "utf8"));
	assert.equal(finalState.calls.filter((call: string[]) => call[0] === "workspace" && call[1] === "create").length, 2);
}));

test("concurrent stale-workspace retries converge on one replacement workspace", async () => fixture("success", async (root, stateFile) => {
	await run(root);
	const state = JSON.parse(fs.readFileSync(stateFile, "utf8")); state.workspace = null; fs.writeFileSync(stateFile, JSON.stringify(state));
	const [first, second] = await Promise.all([run(root, { runId: "stale-a" }), run(root, { runId: "stale-b" })]);
	assert.equal(first.externalExecutionMode, "herdr"); assert.equal(second.externalExecutionMode, "herdr");
	assert.equal(first.externalWorkspaceId, second.externalWorkspaceId);
	const finalState = JSON.parse(fs.readFileSync(stateFile, "utf8"));
	assert.equal(finalState.calls.filter((call: string[]) => call[0] === "workspace" && call[1] === "create").length, 2);
}));

test("keepPanes marks a successful pane instead of closing it", async () => fixture("success", async (root, stateFile) => {
	await run(root, { herdrWorkspace: { workspaceLabel: "subagents", keepPanes: true } }); const state = JSON.parse(fs.readFileSync(stateFile, "utf8"));
	assert.ok(state.calls.some((call: string[]) => call[0] === "agent" && call[1] === "rename" && call[3].startsWith("✓"))); assert.ok(!state.calls.some((call: string[]) => call[0] === "pane" && call[1] === "close" && call[2] === "pane-1"));
}));

test("deep-research topology places two proposers before the aggregator", async () => fixture("success", async (root, stateFile) => {
	const manager = new HerdrWorkspaceManager({ workspaceLabel: "subagents", keepPanes: true });
	const [fable, codex] = await Promise.all([
		manager.startPane({ label: "deep-research-proposer-fable", cwd: root, command: process.execPath, args: ["-e", "process.exit()"] }),
		manager.startPane({ label: "deep-research-proposer-codex", cwd: root, command: process.execPath, args: ["-e", "process.exit()"] }),
	]);
	const aggregator = await manager.startPane({ label: "deep-research-aggregator", cwd: root, command: process.execPath, args: ["-e", "process.exit()"] });
	assert.ok(fable && codex && aggregator); const state = JSON.parse(fs.readFileSync(stateFile, "utf8"));
	const labels = state.calls.filter((call: string[]) => call[0] === "agent" && call[1] === "start").map((call: string[]) => call[2]);
	assert.deepEqual(new Set(labels.slice(0, 2)), new Set(["deep-research-proposer-fable", "deep-research-proposer-codex"])); assert.equal(labels[2], "deep-research-aggregator");
	await Promise.all([manager.interruptPane(fable!), manager.interruptPane(codex!), manager.interruptPane(aggregator!)]);
}));

test("unreachable Herdr falls back locally without changing the result", async () => fixture("success", async (root) => {
	process.env.PI_SUBAGENTS_HERDR_EXECUTABLE = path.join(root, "missing-herdr"); resetHerdrWorkspaceManagerForTests(); const result = await run(root); assert.equal(result.exitCode, 0); assert.equal(result.finalOutput, "external success"); assert.equal(result.externalExecutionMode, "local-fallback");
}));
