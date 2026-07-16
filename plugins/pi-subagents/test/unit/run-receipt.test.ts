import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { compactParentMetadata, formatRunReceipt } from "../../src/runs/shared/run-receipt.ts";

describe("run receipts", () => {
	it("formats fixed-order human-visible fields and never emits prompts", () => {
		const receipt = formatRunReceipt({ kind: "research", backend: "codex-cli", model: "gpt", effort: "high", resumed: false, durationMs: 1234, inputTokens: 1000, outputTokens: 20, contextHash: "abc", cacheReadTokens: 10, cacheWriteTokens: 2, cacheHitRate: 0.5, retries: 1, degraded: false, sourceCount: 3, reportPath: "reports/x.md", snapshotHash: "def", prompt: "TOP SECRET" } as any);
		assert.equal(receipt.split("\n")[0], "Research child receipt");
		assert.match(receipt, /Duration: 1\.23s/);
		assert.match(receipt, /Tokens: 1,000 in \/ 20 out/);
		assert.doesNotMatch(receipt, /TOP SECRET|Prompt:/);
		assert.ok(receipt.indexOf("Backend:") < receipt.indexOf("Model:"));
		assert.match(formatRunReceipt({ kind: "external", executionMode: "herdr", workspaceId: "w1", paneId: "w1:p2" }), /Execution: herdr\nHerdr: w1 \/ w1:p2/);
		assert.match(formatRunReceipt({ kind: "external", executionMode: "local-fallback" }), /Execution: local-fallback/);
	});
	it("uses em dashes and exposes compact parent metadata", () => {
		assert.match(formatRunReceipt({ kind: "advisor" }), /Backend: —/);
		assert.deepEqual(compactParentMetadata({ backend: "pi", model: "m", effort: "low", contextHash: "h", degraded: true }), { backend: "pi", model: "m", effort: "low", contextHash: "h", degraded: true });
	});
});
