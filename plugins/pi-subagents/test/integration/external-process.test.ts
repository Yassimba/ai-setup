import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { describe, it } from "node:test";
import { runExternalProcess } from "../../src/runs/shared/external-process.ts";

const fixture = path.resolve("test/support/external-process-fixture.mjs");
const command = (mode: string, args: string[] = []) => ({ command: process.execPath, args: [fixture, mode, ...args] });

describe("external process fixture", () => {
	it("captures success with stdin, cwd paths containing spaces, and literal arguments", async () => {
		const cwd = fs.mkdtempSync(path.join(os.tmpdir(), "external process space "));
		try {
			const shellPayload = "$(printf shell-injected) ; & | > output";
			const result = await runExternalProcess(command("success", [shellPayload]), { cwd, stdin: "prompt over stdin" });
			assert.equal(result.exitCode, 0);
			assert.equal(result.error, undefined);
			const output = JSON.parse(result.stdout);
			assert.equal(fs.realpathSync(output.cwd), fs.realpathSync(cwd));
			assert.deepEqual({ ...output, cwd }, { cwd, args: [shellPayload], stdin: "prompt over stdin", injected: null });
			assert.equal(fs.existsSync(path.join(cwd, "output")), false);
		} finally { fs.rmSync(cwd, { recursive: true, force: true }); }
	});

	it("supports a prompt argument without shell interpolation", async () => {
		const result = await runExternalProcess(command("success"), { cwd: process.cwd(), prompt: "hello; echo unsafe", promptMode: "argument" });
		assert.deepEqual(JSON.parse(result.stdout).args, ["hello; echo unsafe"]);
		assert.equal(JSON.parse(result.stdout).stdin, "");
	});

	it("returns non-zero exit output", async () => {
		const result = await runExternalProcess(command("fail"), { cwd: process.cwd() });
		assert.equal(result.exitCode, 7);
		assert.equal(result.stderr, "fixture failure");
		assert.match(result.error ?? "", /code 7/);
	});

	it("terminates on timeout", async () => {
		const result = await runExternalProcess(command("wait"), { cwd: process.cwd(), timeoutMs: 30 });
		assert.equal(result.timedOut, true);
		assert.match(result.error ?? "", /timed out/);
		assert.ok(result.elapsedMs < 2_000);
	});

	it("terminates on cancellation", async () => {
		const controller = new AbortController();
		setTimeout(() => controller.abort(), 30);
		const result = await runExternalProcess(command("wait"), { cwd: process.cwd(), signal: controller.signal });
		assert.equal(result.cancelled, true);
		assert.match(result.error ?? "", /cancelled/);
		assert.ok(result.elapsedMs < 2_000);
	});
});
