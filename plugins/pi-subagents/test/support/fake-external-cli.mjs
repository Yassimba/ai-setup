#!/usr/bin/env node
import { appendFileSync } from "node:fs";

if (process.env.FAKE_EXTERNAL_ARGS_FILE) appendFileSync(process.env.FAKE_EXTERNAL_ARGS_FILE, `${JSON.stringify(process.argv.slice(2))}\n`);
if (process.env.FAKE_EXTERNAL_MODE === "wait") {
	setTimeout(() => {}, 10_000);
} else if (process.env.FAKE_EXTERNAL_MODE === "fail") {
	if (process.env.FAKE_EXTERNAL_BACKEND === "codex-cli") {
		console.log(JSON.stringify({ type: "turn.failed", error: { message: "fake failure" } }));
	} else {
		console.log(JSON.stringify({ type: "result", is_error: true, result: "fake failure", session_id: "fake-session" }));
	}
	process.exitCode = 7;
} else if (process.env.FAKE_EXTERNAL_BACKEND === "codex-cli") {
	console.log(JSON.stringify({ type: "thread.started", thread_id: "codex-session", model: "fake model" }));
	console.log(JSON.stringify({ type: "item.completed", item: { type: "agent_message", text: "external success" } }));
	console.log(JSON.stringify({ type: "turn.completed", usage: { input_tokens: 3, output_tokens: 2 } }));
} else {
	const result = process.env.FAKE_EXTERNAL_RESULT ?? "external success";
	console.log(JSON.stringify({ type: "system", subtype: "init", session_id: "claude-session", model: "fake model" }));
	console.log(JSON.stringify({ type: "assistant", message: { model: "fake model", content: [{ type: "text", text: result }], usage: { input_tokens: 3, output_tokens: 2 } } }));
	console.log(JSON.stringify({ type: "result", is_error: false, result, session_id: "claude-session", num_turns: 1, usage: { input_tokens: 3, output_tokens: 2, cache_read_input_tokens: 1 } }));
}
