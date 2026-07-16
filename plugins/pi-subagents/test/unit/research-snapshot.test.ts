import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { execFileSync } from "node:child_process";
import { describe, it } from "node:test";
import { createResearchSnapshot } from "../../src/runs/shared/research-snapshot.ts";

describe("research snapshot", () => {
	it("captures tracked, dirty, and untracked files while excluding unsafe files", () => {
		const repo = fs.mkdtempSync(path.join(os.tmpdir(), "snapshot-repo-"));
		execFileSync("git", ["init", "-q"], { cwd: repo });
		fs.writeFileSync(path.join(repo, ".gitignore"), "ignored.txt\nnode_modules/\n");
		fs.writeFileSync(path.join(repo, "tracked.txt"), "one\n");
		execFileSync("git", ["add", "."], { cwd: repo });
		execFileSync("git", ["-c", "user.name=T", "-c", "user.email=t@e", "commit", "-qm", "init"], { cwd: repo });
		fs.writeFileSync(path.join(repo, "tracked.txt"), "dirty\n");
		fs.writeFileSync(path.join(repo, "new.txt"), "new\n");
		fs.writeFileSync(path.join(repo, ".env"), "TOKEN=x\n");
		fs.writeFileSync(path.join(repo, "ignored.txt"), "ignored\n");
		const first = createResearchSnapshot(repo);
		const second = createResearchSnapshot(repo);
		assert.equal(first.digest, second.digest);
		assert.deepEqual(first.files, [".gitignore", "new.txt", "tracked.txt"]);
		assert.equal(fs.readFileSync(path.join(first.path, "tracked.txt"), "utf8"), "dirty\n");
		assert.equal(fs.existsSync(path.join(first.path, ".env")), false);
		first.cleanup(); second.cleanup();
		assert.equal(fs.existsSync(first.path), false);
	});
});
