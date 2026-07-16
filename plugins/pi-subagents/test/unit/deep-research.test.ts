import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";
import { deepResearchCapabilityRequests, runDeepResearch } from "../../src/runs/shared/deep-research.ts";
import { createCapabilityGrantStore } from "../../src/runs/shared/capability-grants.ts";
import { resetHerdrWorkspaceManagerForTests } from "../../src/runs/shared/herdr-workspace.ts";

const fixture = resolve("test/support/fake-deep-research-cli.mjs");
chmodSync(fixture, 0o755);
const herdrFixture = resolve("test/support/fake-herdr.mjs");
chmodSync(herdrFixture, 0o755);

type Call = { role: string; backend: string; attempt: number; prompt: string; cwd: string; start: number };

type Harness = { repo: string; stateDir: string; agentDir: string; artifactsDir: string; herdrState: string; calls: () => Call[] };

async function withHarness(env: Record<string, string | undefined>, fn: (h: Harness) => Promise<void>): Promise<void> {
	const root = mkdtempSync(join(tmpdir(), "pi-deep-research-"));
	const repo = join(root, "repo");
	mkdirSync(repo, { recursive: true });
	execFileSync("git", ["init", "-q"], { cwd: repo });
	writeFileSync(join(repo, "notes.md"), "local context\n");
	const stateDir = join(root, "state");
	const agentDir = join(root, "agent");
	const artifactsDir = join(root, "artifacts");
	const herdrState = join(root, "herdr.json");
	const codexHome = join(root, "codex-home");
	mkdirSync(codexHome, { recursive: true }); writeFileSync(join(codexHome, "auth.json"), "{}");
	const overrides: Record<string, string | undefined> = {
		CODEX_HOME: codexHome,
		PI_CODING_AGENT_DIR: agentDir,
		PI_SUBAGENTS_CLAUDE_CODE_EXECUTABLE: fixture,
		PI_SUBAGENTS_CODEX_CLI_EXECUTABLE: fixture,
		FAKE_DR_STATE_DIR: stateDir,
		PI_SUBAGENTS_HERDR_EXECUTABLE: herdrFixture,
		FAKE_HERDR_STATE: herdrState,
		FAKE_DR_BARRIER: undefined,
		FAKE_DR_FAIL: undefined,
		FAKE_DR_INVALID: undefined,
		...env,
	};
	const previous = Object.fromEntries(Object.keys(overrides).map((key) => [key, process.env[key]]));
	for (const [key, value] of Object.entries(overrides)) {
		if (value === undefined) delete process.env[key];
		else process.env[key] = value;
	}
	const calls = (): Call[] => {
		try { return readFileSync(join(stateDir, "calls.jsonl"), "utf8").trim().split("\n").map((line) => JSON.parse(line)); } catch { return []; }
	};
	resetHerdrWorkspaceManagerForTests();
	try {
		await fn({ repo, stateDir, agentDir, artifactsDir, herdrState, calls });
	} finally {
		for (const [key, value] of Object.entries(previous)) {
			if (value === undefined) delete process.env[key];
			else process.env[key] = value;
		}
		try { const saved = JSON.parse(readFileSync(herdrState, "utf8")); for (const agent of saved.agents ?? []) if (agent.child_pid) { try { process.kill(-agent.child_pid, "SIGTERM"); } catch {} } } catch {}
		resetHerdrWorkspaceManagerForTests();
		rmSync(root, { recursive: true, force: true });
	}
}

function pregrant(agentDir: string): void {
	const store = createCapabilityGrantStore({ agentDir });
	for (const request of deepResearchCapabilityRequests()) store.grant(request);
}

test("headless deep-research without a pregrant fails closed before any execution", async () => {
	await withHarness({}, async (h) => {
		const outcome = await runDeepResearch({ question: "q", cwd: h.repo, artifactsDir: h.artifactsDir });
		assert.equal(outcome.ok, false);
		assert.equal(outcome.ok === false && outcome.stage, "grants");
		assert.match(outcome.ok === false ? outcome.reason : "", /pregrant/);
		assert.equal(h.calls().length, 0, "no external process may run without grants");
	});
});

test("full run: TUI grants recorded, identical parallel proposer prompts, isolated snapshot, collision-safe report", async () => {
	await withHarness({ FAKE_DR_BARRIER: "1" }, async (h) => {
		let confirmed = 0;
		const outcome = await runDeepResearch({
			question: "How stable is the Fable API?", cwd: h.repo, artifactsDir: h.artifactsDir,
			herdrWorkspace: { workspaceLabel: "subagents", keepPanes: false },
			confirmGrants: async (requests) => { confirmed = requests.length; return true; },
		});
		assert.equal(confirmed, 3, "confirm hook sees all three role/workflow/executable pairs");
		const grants = JSON.parse(readFileSync(join(h.agentDir, "capability-grants.json"), "utf8"));
		assert.equal(grants.grants.length, 3, "exact grants recorded for every pair");
		assert.equal(outcome.ok, true, JSON.stringify(outcome));
		if (!outcome.ok) return;
		assert.equal(outcome.status, "full");
		assert.equal(outcome.retries, 0);
		assert.equal(outcome.sourceCount, 2);
		assert.deepEqual(outcome.gaps, ["gap one", "gap two"]);
		assert.match(outcome.receipt, /deep-research-proposer-fable[\s\S]*deep-research-proposer-codex[\s\S]*deep-research-aggregator[\s\S]*Research child receipt/);

		const calls = h.calls();
		assert.equal(calls.length, 3, "two proposers plus one aggregator, no retries (barrier proves concurrency)");
		const fable = calls.find((c) => c.role === "fable-proposer");
		const codex = calls.find((c) => c.role === "codex-proposer");
		const aggregator = calls.find((c) => c.role === "aggregator");
		assert.ok(fable && codex && aggregator);
		assert.equal(fable.prompt, codex.prompt, "proposer prompts are identical");
		assert.equal(fable.cwd, codex.cwd, "proposers share one immutable snapshot");
		assert.notEqual(fable.cwd, h.repo, "proposers never run in the live repo");
		assert.ok(existsSync(join(fable.cwd, "notes.md")) === false, "snapshot cleaned up after the run");
		assert.ok(!existsSync(fable.cwd), "snapshot directory removed");
		assert.match(aggregator.prompt, /FABLE-REPORT/);
		assert.match(aggregator.prompt, /CODEX-REPORT/);
		assert.doesNotMatch(fable.prompt, /CODEX-REPORT|FABLE-REPORT/, "no proposer sees peer output");
		assert.doesNotMatch(aggregator.prompt, /Degraded run/);
		const herdrState = JSON.parse(readFileSync(h.herdrState, "utf8"));
		const labels = herdrState.calls.filter((call: string[]) => call[0] === "agent" && call[1] === "start").map((call: string[]) => call[2]);
		assert.deepEqual(new Set(labels.slice(0, 2).map((label: string) => label.split("-deep-research-")[0])), new Set(["deep-research-proposer-fable", "deep-research-proposer-codex"]));
		assert.match(labels[2] ?? "", /^deep-research-aggregator-/);

		assert.match(outcome.reportPath, /ai-docs[\\/]research[\\/]deep-research-\d{4}-\d{2}-\d{2}-how-stable/);
		const report = readFileSync(outcome.reportPath, "utf8");
		assert.match(report, /status: full/);
		assert.match(report, /## Gaps/);
		const second = await runDeepResearch({ question: "How stable is the Fable API?", cwd: h.repo, artifactsDir: h.artifactsDir, runId: "second" });
		assert.equal(second.ok, true, JSON.stringify(second));
		if (second.ok) assert.notEqual(second.reportPath, outcome.reportPath, "collision-safe report naming");
	});
});

test("degraded run: failed proposer retried once, aggregation proceeds with survivor and degraded metadata", async () => {
	await withHarness({ FAKE_DR_FAIL: JSON.stringify({ "codex-proposer": 2 }) }, async (h) => {
		pregrant(h.agentDir);
		const outcome = await runDeepResearch({ question: "degraded case", cwd: h.repo, artifactsDir: h.artifactsDir });
		assert.equal(outcome.ok, true, JSON.stringify(outcome));
		if (!outcome.ok) return;
		assert.equal(outcome.status, "degraded");
		assert.equal(outcome.degraded, true);
		assert.match(outcome.degradedReason ?? "", /codex.*failed after retry/);
		assert.equal(outcome.retries, 1, "failed proposer retried exactly once");
		assert.equal(Number(readFileSync(join(h.stateDir, "codex-proposer.attempts"), "utf8")), 2);
		const aggregator = h.calls().find((c) => c.role === "aggregator");
		assert.match(aggregator?.prompt ?? "", /Degraded run/);
		assert.match(aggregator?.prompt ?? "", /FABLE-REPORT/);
		assert.doesNotMatch(aggregator?.prompt ?? "", /CODEX-REPORT/);
		assert.match(readFileSync(outcome.reportPath, "utf8"), /status: degraded/);
	});
});

test("evidence-contract violations are retried and cannot pass as successful research", async () => {
	await withHarness({ FAKE_DR_INVALID: "codex-proposer" }, async (h) => {
		pregrant(h.agentDir);
		const outcome = await runDeepResearch({ question: "invalid evidence", cwd: h.repo, artifactsDir: h.artifactsDir });
		assert.equal(outcome.ok, true, JSON.stringify(outcome));
		if (!outcome.ok) return;
		assert.equal(outcome.status, "degraded");
		assert.match(outcome.degradedReason ?? "", /evidence contract violation/);
		assert.equal(Number(readFileSync(join(h.stateDir, "codex-proposer.attempts"), "utf8")), 2);
	});
});

test("both proposers failing after retries fails the run and cleans the snapshot", async () => {
	await withHarness({ FAKE_DR_FAIL: JSON.stringify({ "codex-proposer": 2, "fable-proposer": 2 }) }, async (h) => {
		pregrant(h.agentDir);
		const outcome = await runDeepResearch({ question: "both fail", cwd: h.repo, artifactsDir: h.artifactsDir });
		assert.equal(outcome.ok, false);
		if (outcome.ok) return;
		assert.equal(outcome.stage, "proposers");
		assert.match(outcome.reason, /both proposers failed/);
		assert.equal(h.calls().filter((c) => c.role === "aggregator").length, 0, "no aggregation without a survivor");
		const snapshotCwd = h.calls()[0]?.cwd;
		assert.ok(snapshotCwd && !existsSync(snapshotCwd), "snapshot cleaned up on failure");
	});
});

test("aggregator failure fails the run but preserves proposer artifacts", async () => {
	await withHarness({ FAKE_DR_FAIL: JSON.stringify({ aggregator: 1 }) }, async (h) => {
		pregrant(h.agentDir);
		const outcome = await runDeepResearch({ question: "aggregator down", cwd: h.repo, artifactsDir: h.artifactsDir, runId: "agg-fail" });
		assert.equal(outcome.ok, false);
		if (outcome.ok) return;
		assert.equal(outcome.stage, "aggregator");
		assert.match(outcome.reason, /aggregator failed/);
		assert.match(outcome.reason, /artifacts preserved/i);
		assert.equal(outcome.artifactsDir, h.artifactsDir);
		const artifactFiles = readdirSync(h.artifactsDir, { recursive: true }) as string[];
		assert.ok(artifactFiles.some((file) => String(file).includes("deep-research-proposer-fable")), "fable proposer artifacts preserved");
		assert.ok(artifactFiles.some((file) => String(file).includes("deep-research-proposer-codex")), "codex proposer artifacts preserved");
	});
});
