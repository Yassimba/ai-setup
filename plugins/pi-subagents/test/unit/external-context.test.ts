import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { canonicalizeExternalContext } from "../../src/runs/shared/external-context.ts";

describe("external context policy", () => {
	it("canonicalizes without mutation and redacts unsafe data", () => {
		const input = {
			workspace: "/work/repo",
			sections: [
				{ kind: "task", priority: 1, value: { files: ["/work/repo/z.ts", "/work/repo/a.ts"], token: "secret", note: "a\r\nb Authorization: Bearer abc.def.ghi api_key=sk_test_1234567890123456" } },
				{ kind: "memory", priority: 9, value: { personalMemory: "private", path: "/home/me/.ssh/id_rsa" } },
			],
			hiddenMessages: ["orchestrator secret"],
		};
		const before = structuredClone(input);
		const result = canonicalizeExternalContext(input);
		assert.deepEqual(input, before);
		assert.equal(result.hash.length, 64);
		assert.equal(result.bytes, Buffer.byteLength(result.text));
		assert.match(result.text, /<redacted:credential>/);
		assert.match(result.text, /<redacted:hidden-orchestration>/);
		assert.match(result.text, /z\.ts.*a\.ts/s, "ordered context such as decision history must retain sequence");
		assert.equal(result.text, canonicalizeExternalContext(input).text);
		assert.doesNotMatch(result.text, /\/work\/repo|abc\.def\.ghi|sk_test_/);
		assert.ok(result.redactionCounts.credential >= 1);
	});

	it("applies deterministic advisor seed and delta caps by section priority", () => {
		const sections = [
			{ kind: "low", priority: 9, value: "l".repeat(250_000) },
			{ kind: "high", priority: 1, value: "h".repeat(250_000) },
		];
		const seed = canonicalizeExternalContext({ workspace: "/w", sections }, { mode: "advisor-seed" });
		const delta = canonicalizeExternalContext({ workspace: "/w", sections }, { mode: "advisor-delta" });
		assert.ok(seed.text.length <= 200_000 && seed.text.includes("high"));
		assert.ok(delta.text.length <= 40_000 && delta.incompleteMarkers.includes("size-cap"));
	});
});
