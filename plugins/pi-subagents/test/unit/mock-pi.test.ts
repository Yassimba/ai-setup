import assert from "node:assert/strict";
import * as fs from "node:fs";
import { describe, it } from "node:test";
import { createMockPi } from "../support/mock-pi.ts";

const PI_BINARY_ENV = "PI_SUBAGENT_PI_BINARY";

describe("createMockPi", () => {
	it("masks and restores an inherited Pi binary override", () => {
		const original = process.env[PI_BINARY_ENV];
		const inheritedOverride = "/opt/real-pi";
		process.env[PI_BINARY_ENV] = inheritedOverride;
		const mockPi = createMockPi();

		try {
			mockPi.install();
			const installedOverride = process.env[PI_BINARY_ENV];
			assert.notEqual(installedOverride, inheritedOverride);
			if (process.platform === "win32") assert.equal(installedOverride, undefined);
			else assert.equal(fs.existsSync(installedOverride ?? ""), true);

			mockPi.uninstall();
			assert.equal(process.env[PI_BINARY_ENV], inheritedOverride);
		} finally {
			mockPi.uninstall();
			if (original === undefined) delete process.env[PI_BINARY_ENV];
			else process.env[PI_BINARY_ENV] = original;
		}
	});
});
