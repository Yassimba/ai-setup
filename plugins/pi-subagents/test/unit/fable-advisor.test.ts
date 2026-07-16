import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import test from "node:test";
import { handleFableAdvisorAction } from "../../src/runs/shared/fable-advisor.ts";
import { resetHerdrWorkspaceManagerForTests } from "../../src/runs/shared/herdr-workspace.ts";

const fixture = path.resolve("test/support/fake-external-cli.mjs");
const herdrFixture = path.resolve("test/support/fake-herdr.mjs");
fs.chmodSync(herdrFixture, 0o755);
const advice = JSON.stringify({ verdict: "continue", crux: "Stay the course", advice: ["Run tests"], evidenceRequests: [], risks: [], recheckAfter: "After tests" });

function entry(role: "user" | "assistant", text: string) {
	return { type: "message", message: { role, content: [{ type: "text", text }] } };
}

function context(root: string, id: string, branch: Array<Record<string, unknown>>, confirm = true) {
	const sessionFile = path.join(root, "sessions", `${id}.jsonl`);
	fs.mkdirSync(path.dirname(sessionFile), { recursive: true });
	fs.writeFileSync(sessionFile, "");
	return {
		cwd: root,
		hasUI: true,
		ui: { confirm: async () => confirm },
		sessionManager: {
			getSessionFile: () => sessionFile,
			getSessionId: () => id,
			getBranch: () => branch,
		},
	} as never;
}

test("advisor is grant-gated, seeds once, resumes with ordered delta, and isolates parent sessions", async () => {
	const root = fs.mkdtempSync(path.join(os.tmpdir(), "fable-advisor-"));
	const previous = {
		agentDir: process.env.PI_CODING_AGENT_DIR,
		executable: process.env.PI_SUBAGENTS_CLAUDE_CODE_EXECUTABLE,
		backend: process.env.FAKE_EXTERNAL_BACKEND,
		result: process.env.FAKE_EXTERNAL_RESULT,
		args: process.env.FAKE_EXTERNAL_ARGS_FILE,
		herdr: process.env.PI_SUBAGENTS_HERDR_EXECUTABLE,
		herdrState: process.env.FAKE_HERDR_STATE,
	};
	const argsFile = path.join(root, "args.jsonl");
	const herdrStateFile = path.join(root, "herdr.json");
	process.env.PI_CODING_AGENT_DIR = path.join(root, "agent");
	process.env.PI_SUBAGENTS_CLAUDE_CODE_EXECUTABLE = fixture;
	process.env.FAKE_EXTERNAL_BACKEND = "claude-code";
	process.env.FAKE_EXTERNAL_RESULT = advice;
	process.env.FAKE_EXTERNAL_ARGS_FILE = argsFile;
	process.env.PI_SUBAGENTS_HERDR_EXECUTABLE = herdrFixture;
	process.env.FAKE_HERDR_STATE = herdrStateFile;
	resetHerdrWorkspaceManagerForTests();
	const invoke = (params: Parameters<typeof handleFableAdvisorAction>[0], ctx: Parameters<typeof handleFableAdvisorAction>[2]) => handleFableAdvisorAction(params, new AbortController().signal, ctx, { workspaceLabel: "subagents", keepPanes: false });
	try {
		const firstBranch = [entry("user", "original goal"), entry("assistant", "decision one")];
		const first = context(root, "parent-a", firstBranch);
		const inactive = await invoke({ action: "advisor.ask", message: "check" }, first);
		assert.equal(inactive.isError, true);
		assert.match(inactive.content[0].type === "text" ? inactive.content[0].text : "", /not activated/);
		assert.equal((await invoke({ action: "advisor.activate" }, first)).isError, undefined);
		const seed = await invoke({ action: "advisor.ask", message: "review" }, first);
		assert.equal(seed.isError, undefined);
		assert.match(seed.content[0].type === "text" ? seed.content[0].text : "", /Execution: herdr/);

		firstBranch.push(entry("user", "new evidence"));
		const resumed = await invoke({ action: "advisor.ask", message: "recheck" }, first);
		assert.equal(resumed.isError, undefined);
		const calls = fs.readFileSync(argsFile, "utf8").trim().split("\n").map((line) => JSON.parse(line) as string[]);
		assert.equal(calls.length, 2);
		assert.ok(!calls[0].includes("--resume"));
		assert.equal(calls[1][calls[1].indexOf("--resume") + 1], "claude-session");
		const deltaPrompt = calls[1][calls[1].indexOf("-p") + 1] ?? "";
		assert.match(deltaPrompt, /new evidence/);
		assert.doesNotMatch(deltaPrompt, /original goal|decision one|constraints-project-guidance/);
		assert.equal(calls[1][calls[1].indexOf("--tools") + 1], "", "advisor remains tool-free");
		assert.ok(calls[1].includes("--json-schema"));

		fs.writeFileSync(path.join(root, "AGENTS.md"), "new project constraint");
		firstBranch[0] = entry("user", "rewritten branch goal");
		const reseeded = await invoke({ action: "advisor.ask", message: "branch changed" }, first);
		assert.equal(reseeded.isError, undefined);
		const afterReseed = fs.readFileSync(argsFile, "utf8").trim().split("\n").map((line) => JSON.parse(line) as string[]);
		assert.equal(afterReseed.length, 3);
		assert.ok(!afterReseed[2].includes("--resume"));
		assert.match(afterReseed[2][afterReseed[2].indexOf("-p") + 1], /new project constraint.*rewritten branch goal.*decision one.*new evidence/s);
		const limited = await invoke({ action: "advisor.ask", message: "too many" }, first);
		assert.equal(limited.isError, true);
		assert.match(limited.content[0].type === "text" ? limited.content[0].text : "", /limit reached/);

		const other = context(root, "parent-b", [entry("user", "other")]);
		const isolated = await invoke({ action: "advisor.status" }, other);
		assert.equal(isolated.isError, true);
		assert.match(isolated.content[0].type === "text" ? isolated.content[0].text : "", /not activated/);
	} finally {
		const herdrState = JSON.parse(fs.readFileSync(herdrStateFile, "utf8"));
		const labels = herdrState.calls.filter((call: string[]) => call[0] === "agent" && call[1] === "start").map((call: string[]) => call[2]);
		assert.equal(labels.length, 3);
		assert.ok(labels.every((label: string) => label.startsWith("fable-advisor-advisor-")));
		for (const [key, value] of Object.entries(previous)) {
			const env = ({ agentDir: "PI_CODING_AGENT_DIR", executable: "PI_SUBAGENTS_CLAUDE_CODE_EXECUTABLE", backend: "FAKE_EXTERNAL_BACKEND", result: "FAKE_EXTERNAL_RESULT", args: "FAKE_EXTERNAL_ARGS_FILE", herdr: "PI_SUBAGENTS_HERDR_EXECUTABLE", herdrState: "FAKE_HERDR_STATE" } as const)[key as keyof typeof previous];
			if (value === undefined) delete process.env[env]; else process.env[env] = value;
		}
		resetHerdrWorkspaceManagerForTests();
		fs.rmSync(root, { recursive: true, force: true });
	}
});
