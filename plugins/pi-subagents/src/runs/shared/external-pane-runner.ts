#!/usr/bin/env node
import { spawn } from "node:child_process";
import * as fs from "node:fs";
import * as path from "node:path";
import { toSpawnableCommand } from "./executable-resolver.ts";
import type { CommandSpec } from "./external-backends.ts";
import type { ExternalProcessResult } from "./external-process.ts";

type Envelope = { spec: CommandSpec; cwd: string; resultPath: string };

function atomicWrite(file: string, value: unknown): void {
	const temporary = `${file}.${process.pid}.tmp`;
	fs.writeFileSync(temporary, `${JSON.stringify(value)}\n`, { encoding: "utf8", mode: 0o600 });
	fs.renameSync(temporary, file);
}

const envelopePath = process.argv[2];
if (!envelopePath) process.exit(2);
const startedAt = Date.now();
let envelope: Envelope;
try {
	envelope = JSON.parse(fs.readFileSync(envelopePath, "utf8")) as Envelope;
	fs.rmSync(envelopePath, { force: true });
} catch (error) {
	process.stderr.write(`Unable to read private external-run envelope: ${(error as Error).message}\n`);
	process.exit(2);
}
fs.mkdirSync(path.dirname(envelope.resultPath), { recursive: true, mode: 0o700 });
let stdout = "", stderr = "";
try {
	// Windows .cmd/.bat shims run through an escaped cmd.exe command line.
	const spawnable = toSpawnableCommand(envelope.spec.command, envelope.spec.args);
	const child = spawn(spawnable.command, spawnable.args, {
		cwd: envelope.cwd, env: envelope.spec.env ? { ...process.env, ...envelope.spec.env } : process.env,
		shell: false, ...(spawnable.windowsVerbatimArguments ? { windowsVerbatimArguments: true } : {}),
		stdio: ["ignore", "pipe", "pipe"], detached: false, windowsHide: true,
	});
	child.stdout.setEncoding("utf8"); child.stderr.setEncoding("utf8");
	child.stdout.on("data", (chunk: string) => { stdout += chunk; process.stdout.write(chunk); });
	child.stderr.on("data", (chunk: string) => { stderr += chunk; process.stderr.write(chunk); });
	child.on("error", (error) => finish({ exitCode: null, signal: null, error: `Unable to run external command: ${error.message}` }));
	child.on("close", (exitCode, signal) => finish({ exitCode, signal, ...(exitCode ? { error: `External command exited with code ${exitCode}.` } : {}) }));
} catch (error) { finish({ exitCode: null, signal: null, error: `Unable to start external command: ${(error as Error).message}` }); }

let done = false;
function finish(patch: Pick<ExternalProcessResult, "exitCode" | "signal"> & { error?: string }): void {
	if (done) return; done = true;
	atomicWrite(envelope.resultPath, { stdout, stderr, ...patch, elapsedMs: Date.now() - startedAt, timedOut: false, cancelled: false } satisfies ExternalProcessResult);
	// Keep the pane process alive until the parent either closes it or deliberately
	// retains it for inspection. Without this hold Herdr removes the pane as soon
	// as the short-lived external CLI exits, racing receipt normalization/rename.
	setInterval(() => {}, 60_000);
}
