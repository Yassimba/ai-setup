#!/usr/bin/env node
// Fake external CLI for deep-research tests. Distinguishes roles from argv alone:
// Codex invocations contain the `exec` subcommand; Claude carries --effort max (proposer) or xhigh (aggregator).
import { appendFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const args = process.argv.slice(2);
const backend = args.includes("exec") ? "codex-cli" : "claude-code";
const effort = backend === "claude-code" ? args[args.indexOf("--effort") + 1] : "ultra";
const role = backend === "codex-cli" ? "codex-proposer" : effort === "xhigh" ? "aggregator" : "fable-proposer";
const prompt = backend === "codex-cli" ? args[args.length - 1] : args[args.indexOf("-p") + 1];
const stateDir = process.env.FAKE_DR_STATE_DIR;
if (!stateDir) throw new Error("FAKE_DR_STATE_DIR is required");
mkdirSync(stateDir, { recursive: true });

const attemptsFile = join(stateDir, `${role}.attempts`);
const attempt = (existsSync(attemptsFile) ? Number(readFileSync(attemptsFile, "utf8")) : 0) + 1;
writeFileSync(attemptsFile, String(attempt));
appendFileSync(join(stateDir, "calls.jsonl"), `${JSON.stringify({ role, backend, attempt, prompt, cwd: process.cwd(), start: Date.now(), args })}\n`);

function sleep(ms) {
	Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
}

const failing = JSON.parse(process.env.FAKE_DR_FAIL ?? "{}");
const shouldFail = attempt <= (failing[role] ?? 0);

// Barrier: each proposer waits for its peer's start marker, proving concurrent execution.
if (!shouldFail && process.env.FAKE_DR_BARRIER === "1" && role !== "aggregator") {
	writeFileSync(join(stateDir, `${role}.started`), "1");
	const peer = join(stateDir, `${role === "fable-proposer" ? "codex-proposer" : "fable-proposer"}.started`);
	const deadline = Date.now() + 3000;
	while (!existsSync(peer) && Date.now() < deadline) sleep(25);
	if (!existsSync(peer)) {
		console.log(JSON.stringify({ type: "result", is_error: true, result: `barrier timeout for ${role}`, session_id: "s" }));
		process.exit(7);
	}
}

if (shouldFail) {
	if (backend === "codex-cli") console.log(JSON.stringify({ type: "turn.failed", error: { message: `${role} injected failure` } }));
	else console.log(JSON.stringify({ type: "result", is_error: true, result: `${role} injected failure`, session_id: "s" }));
	process.exit(7);
}

const reports = {
	"fable-proposer": "FABLE-REPORT\n\n## Summary\nIndependent result.\n\n## Findings\n1. **Fact — A** — x. [A](https://example.com/fable) (Pub, docs, primary, confidence: high)\n\n## Sources\n- Kept: A\n- Rejected: weak blog\n\n## Gaps\n- none",
	"codex-proposer": "CODEX-REPORT\n\n## Summary\nIndependent result.\n\n## Findings\n1. **Fact — B** — y. [B](https://example.com/codex) (Pub, spec, primary, confidence: medium)\n\n## Sources\n- Kept: B\n- Rejected: weak blog\n\n## Gaps\n- none",
	aggregator: "# Research: final\n\n## Summary\nAggregated answer.\n\n## Findings\n1. **Fact — A** — x. [A](https://example.com/a) (Pub, docs, primary, confidence: high)\n2. **Inference — B** — y. [B](https://example.com/b) (Pub, spec, primary, confidence: medium)\n\n## Sources\n- Kept: [A](https://example.com/a) — Pub, docs, primary, current\n- Rejected: C (https://example.com/c) — SEO spam\n\n## Gaps\n- gap one\n- gap two",
};
const report = process.env.FAKE_DR_INVALID === role ? "not an evidence-contract report" : reports[role];
if (backend === "codex-cli") {
	console.log(JSON.stringify({ type: "thread.started", thread_id: "codex-session", model: "gpt-5.6-sol" }));
	console.log(JSON.stringify({ type: "item.completed", item: { type: "agent_message", text: report } }));
	console.log(JSON.stringify({ type: "turn.completed", usage: { input_tokens: 3, output_tokens: 2 } }));
} else {
	console.log(JSON.stringify({ type: "system", subtype: "init", session_id: "claude-session", model: "fable" }));
	console.log(JSON.stringify({ type: "assistant", message: { model: "fable", content: [{ type: "text", text: report }], usage: { input_tokens: 3, output_tokens: 2 } } }));
	console.log(JSON.stringify({ type: "result", is_error: false, result: report, session_id: "claude-session", num_turns: 1, usage: { input_tokens: 3, output_tokens: 2 } }));
}
