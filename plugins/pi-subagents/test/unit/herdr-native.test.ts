import assert from "node:assert/strict";
import test from "node:test";
import {
	gridSlotFor,
	resolveHerdrWorkspaceSetting,
	toNativeArgs,
} from "../../src/runs/background/herdr-native.ts";

test("resolveHerdrWorkspaceSetting handles boolean and object forms", () => {
	assert.deepEqual(resolveHerdrWorkspaceSetting(true), {
		workspaceLabel: "subagents",
		keepPanes: false,
	});
	assert.equal(resolveHerdrWorkspaceSetting(undefined), undefined);
	assert.equal(resolveHerdrWorkspaceSetting(false), undefined);
	assert.equal(resolveHerdrWorkspaceSetting({ enabled: false, workspaceLabel: "x" }), undefined);
	assert.deepEqual(resolveHerdrWorkspaceSetting({}), {
		workspaceLabel: "subagents",
		keepPanes: false,
	});
	assert.deepEqual(resolveHerdrWorkspaceSetting({ workspaceLabel: "my agents", keepPanes: true }), {
		workspaceLabel: "my agents",
		keepPanes: true,
	});
	assert.deepEqual(resolveHerdrWorkspaceSetting({ workspaceLabel: "  " }), {
		workspaceLabel: "subagents",
		keepPanes: false,
	});
});

test("toNativeArgs strips managed-mode flags and pins the session file", () => {
	const managed = [
		"--mode", "json", "-p",
		"--session", "/old/session.jsonl",
		"--model", "prov/model:low",
		"--append-system-prompt", "/tmp/prompt.md",
		"Task: do it",
	];
	assert.deepEqual(toNativeArgs(managed, "/runs/native.jsonl"), [
		"--session", "/runs/native.jsonl",
		"--model", "prov/model:low",
		"--append-system-prompt", "/tmp/prompt.md",
		"Task: do it",
	]);
});

test("toNativeArgs drops session-dir and --print variants too", () => {
	assert.deepEqual(
		toNativeArgs(["--session-dir", "/dir", "--print", "--mode", "json", "hello"], "/s.jsonl"),
		["--session", "/s.jsonl", "hello"],
	);
});

test("gridSlotFor fills a two-row grid: 2x2 first, then side columns", () => {
	const slots = [1, 2, 3, 4, 5, 6, 7, 8].map((n) => gridSlotFor(n));
	assert.deepEqual(slots, [
		{ row: "top", column: 0 },
		{ row: "top", column: 1 },
		{ row: "bottom", column: 0 },
		{ row: "bottom", column: 1 },
		{ row: "top", column: 2 },
		{ row: "bottom", column: 2 },
		{ row: "top", column: 3 },
		{ row: "bottom", column: 3 },
	]);
});
