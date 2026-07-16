import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { describe, it } from "node:test";
import { createCapabilityGrantStore, sha256 } from "../../src/runs/shared/capability-grants.ts";

describe("capability grants", () => {
	it("persists atomically and verifies every bound capability value", () => {
		const root = fs.mkdtempSync(path.join(os.tmpdir(), "grants-"));
		const executable = path.join(root, "bin", "tool");
		fs.mkdirSync(path.dirname(executable), { recursive: true });
		fs.writeFileSync(executable, "tool");
		const store = createCapabilityGrantStore({ agentDir: path.join(root, "agent"), now: () => 123 });
		const request = { role: "researcher", roleContent: "role v1", workflow: "deep", workflowContent: "flow v1", backend: "codex-cli", executable };
		const grant = store.grant(request);
		assert.equal(grant.roleDigest, sha256("role v1"));
		assert.equal(grant.workflowDigest, sha256("flow v1"));
		assert.equal(grant.executableDigest, sha256(fs.realpathSync(executable)));
		assert.equal(store.verify(request).allowed, true);
		assert.equal(store.verify({ ...request, roleContent: "role v2" }).allowed, false);
		assert.equal(store.verify({ ...request, workflow: "other" }).allowed, false);
		assert.equal(store.verify({ ...request, backend: "claude-code" }).allowed, false);
		fs.writeFileSync(executable, "changed tool");
		assert.equal(store.verify(request).allowed, true, "in-place CLI upgrades keep the canonical executable identity");
		assert.ok(fs.existsSync(path.join(root, "agent", "capability-grants.json")));
		assert.equal(fs.readdirSync(path.join(root, "agent")).some((name) => name.endsWith(".tmp")), false);
	});
});
