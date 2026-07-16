import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { describe, it } from "node:test";
import {
	executableNeedsShell,
	resolveExecutable,
	toSpawnableCommand,
	windowsExecutableCandidates,
} from "../../src/runs/shared/executable-resolver.ts";

function fakeFs(files: string[]) {
	const set = new Set(files);
	return {
		statSync: (target: string) => {
			if (!set.has(target)) throw Object.assign(new Error("ENOENT"), { code: "ENOENT" });
			return { isFile: () => true };
		},
		realpathSync: (target: string) => target,
	};
}

describe("windowsExecutableCandidates", () => {
	it("orders .exe before .cmd/.bat and never includes the bare name", () => {
		const candidates = windowsExecutableCandidates("claude", { PATHEXT: ".COM;.EXE;.BAT;.CMD;.JS" });
		assert.equal(candidates.includes("claude"), false);
		const lower = candidates.filter((name) => name === name.toLowerCase());
		assert.deepEqual(lower, ["claude.exe", "claude.cmd", "claude.bat", "claude.com", "claude.js"]);
	});

	it("falls back to .EXE/.CMD/.BAT when PATHEXT is unset", () => {
		const candidates = windowsExecutableCandidates("codex", {});
		assert.deepEqual(candidates.filter((name) => name === name.toLowerCase()), ["codex.exe", "codex.cmd", "codex.bat"]);
	});
});

describe("resolveExecutable on win32", () => {
	const env = { PATH: ["/win", "/other"].join(path.delimiter), PATHEXT: ".EXE;.CMD;.BAT" };

	it("skips the extension-less npm sh script and picks the .cmd shim", () => {
		const resolved = resolveExecutable("claude", { platform: "win32", env, ...fakeFs(["/win/claude", "/win/claude.cmd"]) });
		assert.equal(resolved.path, "/win/claude.cmd");
		assert.equal(resolved.needsShell, true);
	});

	it("prefers .exe over .cmd in the same directory", () => {
		const resolved = resolveExecutable("claude", { platform: "win32", env, ...fakeFs(["/win/claude.cmd", "/win/claude.exe"]) });
		assert.equal(resolved.path, "/win/claude.exe");
		assert.equal(resolved.needsShell, false);
	});

	it("finds a .cmd in a later PATH directory instead of an earlier sh script", () => {
		const resolved = resolveExecutable("codex", { platform: "win32", env, ...fakeFs(["/win/codex", "/other/codex.cmd"]) });
		assert.equal(resolved.path, "/other/codex.cmd");
		assert.equal(resolved.needsShell, true);
	});

	it("never resolves to an extension-less file even when it is the only match", () => {
		const resolved = resolveExecutable("claude", { platform: "win32", env, ...fakeFs(["/win/claude"]) });
		assert.equal(resolved.path, "claude", "falls back to the configured name instead of the sh script");
	});

	it("honors an explicit absolute .cmd override (PI_SUBAGENTS_*_EXECUTABLE)", () => {
		const resolved = resolveExecutable("/tools/claude.cmd", { platform: "win32", env, ...fakeFs(["/tools/claude.cmd"]) });
		assert.equal(resolved.path, path.resolve("/tools/claude.cmd"));
		assert.equal(resolved.needsShell, true);
	});

	it("probes extensions for an explicit extension-less path", () => {
		const resolved = resolveExecutable("/tools/claude", { platform: "win32", env, ...fakeFs(["/tools/claude", "/tools/claude.cmd"]) });
		assert.equal(resolved.path, `${path.resolve("/tools/claude")}.cmd`);
		assert.equal(resolved.needsShell, true);
	});

	it("probes a bare name that already carries an extension as-is first", () => {
		const resolved = resolveExecutable("claude.cmd", { platform: "win32", env, ...fakeFs(["/win/claude.cmd"]) });
		assert.equal(resolved.path, "/win/claude.cmd");
		assert.equal(resolved.needsShell, true);
	});
});

describe("resolveExecutable on unix", () => {
	it("returns the realpath of the first plain PATH match", () => {
		const root = fs.mkdtempSync(path.join(os.tmpdir(), "exec-resolver-"));
		try {
			const file = path.join(root, "claude");
			fs.writeFileSync(file, "#!/bin/sh\n", { mode: 0o755 });
			const resolved = resolveExecutable("claude", { platform: "linux", env: { PATH: root } });
			assert.equal(resolved.path, fs.realpathSync(file));
			assert.equal(resolved.needsShell, false);
		} finally {
			fs.rmSync(root, { recursive: true, force: true });
		}
	});

	it("resolves explicit paths without probing the filesystem", () => {
		const resolved = resolveExecutable("./bin/claude", { platform: "linux", env: { PATH: "" } });
		assert.equal(resolved.path, path.resolve("./bin/claude"));
		assert.equal(resolved.needsShell, false);
	});

	it("returns the configured name unchanged when nothing matches", () => {
		const resolved = resolveExecutable("claude", { platform: "linux", env: { PATH: "/definitely-missing" } });
		assert.deepEqual(resolved, { path: "claude", needsShell: false });
	});
});

describe("toSpawnableCommand", () => {
	it("passes non-shell commands through untouched", () => {
		assert.deepEqual(toSpawnableCommand("/bin/claude", ["-p", "hi"], { platform: "linux" }), { command: "/bin/claude", args: ["-p", "hi"] });
		assert.deepEqual(toSpawnableCommand("C:\\t\\claude.exe", ["-p"], { platform: "win32" }), { command: "C:\\t\\claude.exe", args: ["-p"] });
	});

	it("routes .cmd shims through cmd.exe with a quoted command line", () => {
		const spawnable = toSpawnableCommand("/tools/claude.cmd", ["-p", "plain"], { platform: "win32", env: {} });
		assert.equal(spawnable.command, "cmd.exe");
		assert.equal(spawnable.windowsVerbatimArguments, true);
		assert.deepEqual(spawnable.args.slice(0, 3), ["/d", "/s", "/c"]);
		// Each argument is quoted, then caret-escaped twice for the npm shim's %* re-parse.
		assert.equal(spawnable.args[3], `"${path.normalize("/tools/claude.cmd")} ^^^"-p^^^" ^^^"plain^^^""`);
	});

	it("respects a configured comspec", () => {
		const spawnable = toSpawnableCommand("x.bat", [], { platform: "win32", env: { comspec: "C:\\WINDOWS\\system32\\cmd.exe" } });
		assert.equal(spawnable.command, "C:\\WINDOWS\\system32\\cmd.exe");
	});

	it("escapes cmd metacharacters in user-controlled arguments", () => {
		const hostile = 'question" & del C:\\ & echo "%PATH%';
		const spawnable = toSpawnableCommand("claude.cmd", ["-p", hostile], { platform: "win32", env: {} });
		const commandLine = spawnable.args[3] ?? "";
		// Every quote, ampersand, and percent from the argument is caret-escaped;
		// nothing can terminate the quoted argument and start a new command.
		assert.doesNotMatch(commandLine.slice(1, -1), /(?<!\^)"(?! )/, "no bare quote survives inside the command line");
		assert.doesNotMatch(commandLine, /(?<!\^)&/, "no unescaped ampersand");
		assert.doesNotMatch(commandLine, /(?<!\^)%/, "no unescaped percent");
	});
});

describe("executableNeedsShell", () => {
	it("flags .cmd/.bat only on win32", () => {
		assert.equal(executableNeedsShell("claude.cmd", "win32"), true);
		assert.equal(executableNeedsShell("claude.BAT", "win32"), true);
		assert.equal(executableNeedsShell("claude.exe", "win32"), false);
		assert.equal(executableNeedsShell("claude.cmd", "linux"), false);
	});
});
