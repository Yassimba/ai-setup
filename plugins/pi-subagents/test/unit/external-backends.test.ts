import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
	buildClaudeCommand,
	buildCodexCommand,
	mapExternalTools,
	normalizeExternalTools,
	selectBackend,
} from "../../src/runs/shared/external-backends.ts";

describe("external backend command builders", () => {
	it("defaults backend selection to pi and preserves explicit backends", () => {
		assert.equal(selectBackend(undefined), "pi");
		assert.equal(selectBackend("claude-code"), "claude-code");
		assert.equal(selectBackend("codex-cli"), "codex-cli");
	});

	it("validates and maps the external tool vocabulary", () => {
		assert.deepEqual(normalizeExternalTools([" read ", "grep", "read", "web_search", "fetch_content"]), ["read", "grep", "web_search", "fetch_content"]);
		assert.deepEqual(mapExternalTools("claude-code", ["read", "grep", "find", "web_search", "fetch_content"]), ["Read", "Grep", "Glob", "WebSearch", "WebFetch"]);
		assert.deepEqual(normalizeExternalTools([]), []);
		assert.throws(() => normalizeExternalTools(["bash"]), /Unknown external tool 'bash'/);
	});

	it("builds a safe ephemeral Claude stream-json command", () => {
		assert.deepEqual(buildClaudeCommand({
			executable: "/opt/bin/claude",
			prompt: "research this",
			tools: ["read", "find", "web_search"],
			model: "claude-opus-4-6",
			effort: "high",
			permissionMode: "plan",
			sessionMode: "ephemeral",
		}), {
			command: "/opt/bin/claude",
			args: ["-p", "research this", "--output-format", "stream-json", "--verbose", "--safe-mode", "--prompt-suggestions", "false", "--permission-mode", "plan", "--strict-mcp-config", "--tools", "Read,Glob,WebSearch", "--no-session-persistence", "--model", "claude-opus-4-6", "--effort", "high"],
		});
	});

	it("builds a resumable Claude command without disabling persistence", () => {
		const spec = buildClaudeCommand({ executable: "claude", prompt: "inspect", tools: [], permissionMode: "dontAsk", sessionMode: "resumable" });
		assert.deepEqual(spec.args, ["-p", "inspect", "--output-format", "stream-json", "--verbose", "--safe-mode", "--prompt-suggestions", "false", "--permission-mode", "dontAsk", "--strict-mcp-config", "--tools", ""]);
	});

	it("builds safe Codex exec and resume commands", () => {
		assert.deepEqual(buildCodexCommand({ executable: "custom-codex", prompt: "inspect", tools: ["read", "web_search"], model: "gpt-5.3-codex", reasoningEffort: "high", ephemeral: true }), {
			command: "custom-codex",
			args: ["--ask-for-approval", "never", "exec", "--json", "--sandbox", "read-only", "--ignore-user-config", "--ignore-rules", "-c", "project_doc_max_bytes=0", "--ephemeral", "-c", "web_search=\"live\"", "--model", "gpt-5.3-codex", "-c", "model_reasoning_effort=\"high\"", "inspect"],
		});
		const resumed = buildCodexCommand({ executable: "codex", prompt: "continue", tools: [], resumeSessionId: "session-1", ephemeral: false });
		assert.deepEqual(resumed.args.slice(0, 5), ["--ask-for-approval", "never", "exec", "resume", "session-1"]);
		assert.ok(resumed.args.includes("read-only"));
		assert.ok(!resumed.args.includes("--search"));
		assert.ok(!resumed.args.some((arg) => arg.includes("dangerously")));
	});
});
