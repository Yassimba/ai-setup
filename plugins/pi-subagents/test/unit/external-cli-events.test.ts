import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { ExternalCliEventNormalizer, normalizeExternalCliJsonl } from "../../src/runs/shared/external-cli-events.ts";

const claudeFixture = [
	'{"type":"system","subtype":"init","cwd":"/tmp/project","session_id":"claude-session","tools":["Read"],"model":"claude-sonnet-4-5-20250929"}',
	'{"type":"assistant","message":{"model":"claude-sonnet-4-5-20250929","id":"msg_1","type":"message","role":"assistant","content":[{"type":"text","text":"Checking. "},{"type":"tool_use","id":"toolu_1","name":"Read","input":{"file_path":"README.md"}}],"usage":{"input_tokens":10,"cache_creation_input_tokens":3,"cache_read_input_tokens":4,"output_tokens":2}},"session_id":"claude-session"}',
	'{"type":"user","message":{"role":"user","content":[{"tool_use_id":"toolu_1","type":"tool_result","content":"hello"}]},"session_id":"claude-session"}',
	'{"type":"assistant","message":{"model":"claude-sonnet-4-5-20250929","content":[{"type":"text","text":"Done."}],"usage":{"input_tokens":5,"cache_creation_input_tokens":0,"cache_read_input_tokens":2,"output_tokens":3}},"session_id":"claude-session"}',
	'{"type":"result","subtype":"success","is_error":false,"duration_ms":1250,"duration_api_ms":1000,"num_turns":2,"result":"Checking. Done.","session_id":"claude-session","total_cost_usd":0.01,"usage":{"input_tokens":15,"cache_creation_input_tokens":3,"cache_read_input_tokens":6,"output_tokens":5}}',
].join("\n");

const codexFixture = [
	'{"type":"thread.started","thread_id":"019c-test"}',
	'{"type":"turn.started"}',
	'{"type":"item.started","item":{"id":"item_0","type":"command_execution","command":"cat README.md","status":"in_progress"}}',
	'{"type":"item.completed","item":{"id":"item_0","type":"command_execution","command":"cat README.md","aggregated_output":"hello\\n","exit_code":0,"status":"completed"}}',
	'{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"The answer is hello."}}',
	'{"type":"turn.completed","usage":{"input_tokens":100,"cached_input_tokens":80,"output_tokens":20}}',
].join("\n");

describe("external CLI event normalization", () => {
	it("normalizes current Claude Code stream-json", () => {
		const result = normalizeExternalCliJsonl("claude-code", claudeFixture);
		assert.equal(result.summary.finalOutput, "Checking. Done.");
		assert.equal(result.summary.sessionId, "claude-session");
		assert.equal(result.summary.model, "claude-sonnet-4-5-20250929");
		assert.deepEqual(result.summary.usage, { input: 15, output: 5, cacheRead: 6, cacheWrite: 3 });
		assert.equal(result.summary.turns, 2);
		assert.equal(result.summary.elapsedMs, 1250);
		assert.deepEqual(result.events.map((event) => event.type), ["assistant_text", "tool_start", "tool_result", "assistant_text"]);
	});

	it("uses Claude structured output as the terminal result", () => {
		const result = normalizeExternalCliJsonl("claude-code", '{"type":"result","subtype":"success","is_error":false,"structured_output":{"verdict":"continue","advice":[]},"session_id":"s"}');
		assert.equal(result.summary.finalOutput, '{"verdict":"continue","advice":[]}');
		assert.equal(result.summary.terminal, true);
	});

	it("normalizes current Codex exec --json JSONL", () => {
		const result = normalizeExternalCliJsonl("codex-cli", codexFixture, 900);
		assert.equal(result.summary.finalOutput, "The answer is hello.");
		assert.equal(result.summary.sessionId, "019c-test");
		assert.deepEqual(result.summary.usage, { input: 100, output: 20, cacheRead: 80, cacheWrite: 0 });
		assert.equal(result.summary.elapsedMs, 900);
		assert.deepEqual(result.events.map((event) => event.type), ["tool_start", "tool_result", "assistant_text"]);
	});

	it("bounds malformed diagnostics, excludes them from output, and makes terminal events idempotent", () => {
		const normalizer = new ExternalCliEventNormalizer("codex-cli");
		for (let index = 0; index < 30; index++) normalizer.pushLine(`not-json-${index}-${"x".repeat(600)}`);
		normalizer.pushLine('{"type":"item.completed","item":{"id":"a","type":"agent_message","text":"trusted"}}');
		normalizer.pushLine('{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":2}}');
		normalizer.pushLine('{"type":"turn.completed","usage":{"input_tokens":999,"output_tokens":999}}');
		const result = normalizer.finish();
		assert.equal(result.diagnostics.length, 20);
		assert.ok(result.diagnostics.every((diagnostic) => diagnostic.length <= 500));
		assert.equal(result.summary.finalOutput, "trusted");
		assert.deepEqual(result.summary.usage, { input: 1, output: 2, cacheRead: 0, cacheWrite: 0 });
	});

	it("captures terminal errors without treating error text as final advice", () => {
		const result = normalizeExternalCliJsonl("claude-code", '{"type":"result","subtype":"error_during_execution","is_error":true,"result":"credential rejected","session_id":"s"}');
		assert.equal(result.summary.error, "credential rejected");
		assert.equal(result.summary.finalOutput, "");
		assert.equal(result.events[0]?.type, "error");
	});
});
