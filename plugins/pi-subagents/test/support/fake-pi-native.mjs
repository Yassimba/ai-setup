#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";

const args = process.argv.slice(2);
const sessionIndex = args.indexOf("--session");
const sessionFile = sessionIndex >= 0 ? args[sessionIndex + 1] : undefined;
if (!sessionFile) throw new Error("--session is required");
fs.mkdirSync(path.dirname(sessionFile), { recursive: true });
fs.appendFileSync(sessionFile, `${JSON.stringify({
	type: "message",
	message: {
		role: "assistant",
		content: [{ type: "text", text: "native herdr works" }],
		stopReason: "stop",
		model: "fake/native",
		usage: { input: 2, output: 3, cacheRead: 0, cacheWrite: 0, cost: { total: 0 } },
	},
})}\n`);
setInterval(() => {}, 60_000);
