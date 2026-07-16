#!/usr/bin/env node
const [mode, ...args] = process.argv.slice(2);
if (mode === "success") {
	let stdin = "";
	process.stdin.setEncoding("utf8");
	for await (const chunk of process.stdin) stdin += chunk;
	process.stdout.write(JSON.stringify({ cwd: process.cwd(), args, stdin, injected: process.env.EXTERNAL_FIXTURE_INJECTED ?? null }));
} else if (mode === "fail") {
	process.stderr.write("fixture failure");
	process.exitCode = 7;
} else if (mode === "wait") {
	setTimeout(() => process.stdout.write("too late"), 10_000);
} else {
	process.stderr.write("unknown fixture mode");
	process.exitCode = 2;
}
