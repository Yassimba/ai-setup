import assert from "node:assert/strict";
import { it } from "node:test";
import { runExternalProcess } from "../../src/runs/shared/external-process.ts";

it("returns cancellation without spawning when the signal is already aborted", async () => {
	const controller = new AbortController();
	controller.abort();
	const result = await runExternalProcess({ command: "definitely-not-a-command", args: [], env: { SECRET_TOKEN: "do-not-print" } }, { cwd: process.cwd(), signal: controller.signal });
	assert.equal(result.cancelled, true);
	assert.equal(result.exitCode, null);
	assert.doesNotMatch(result.error ?? "", /SECRET_TOKEN|do-not-print/);
});

it("normalizes spawn errors without exposing environment values", async () => {
	const secret = "unique-secret-value";
	const result = await runExternalProcess({ command: "missing-external-process-command", args: [], env: { API_TOKEN: secret } }, { cwd: process.cwd() });
	assert.equal(result.exitCode, null);
	assert.match(result.error ?? "", /Unable to run/);
	assert.doesNotMatch(result.error ?? "", new RegExp(secret));
});
