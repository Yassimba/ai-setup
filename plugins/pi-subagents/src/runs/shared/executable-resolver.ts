import * as fs from "node:fs";
import * as path from "node:path";

/**
 * Cross-platform executable resolution for external CLIs (claude, codex,
 * typescript-language-server, ...). On Windows, npm installs a CLI as `.cmd`
 * and `.ps1` shims next to an extension-less POSIX sh script; the sh script
 * can never be spawned by Node, so resolution probes PATHEXT candidates
 * (preferring `.exe`, then `.cmd`/`.bat`) and never selects an extension-less
 * file. On other platforms the behavior matches a plain PATH scan.
 */

export interface ResolvedExecutable {
	/** Resolved path, or the configured value unchanged when nothing resolved. */
	path: string;
	/** True when the file is a Windows .cmd/.bat that only cmd.exe can run. */
	needsShell: boolean;
}

export interface SpawnableCommand {
	command: string;
	args: string[];
	/** Set when the command line is pre-quoted for cmd.exe. */
	windowsVerbatimArguments?: boolean;
}

export type ExecutableResolverDeps = {
	platform?: NodeJS.Platform;
	env?: NodeJS.ProcessEnv;
	statSync?: (target: string) => { isFile(): boolean };
	realpathSync?: (target: string) => string;
};

const WINDOWS_SHELL_EXTENSION = /\.(?:cmd|bat)$/i;
const DEFAULT_PATHEXT = ".EXE;.CMD;.BAT";

export function executableNeedsShell(filePath: string, platform: NodeJS.Platform = process.platform): boolean {
	return platform === "win32" && WINDOWS_SHELL_EXTENSION.test(filePath);
}

function windowsExtensions(env: NodeJS.ProcessEnv): string[] {
	const raw = (env.PATHEXT ?? DEFAULT_PATHEXT).split(";").map((ext) => ext.trim()).filter((ext) => ext.startsWith("."));
	const upper = [...new Set(raw.map((ext) => ext.toUpperCase()))];
	const preferred = [".EXE", ".CMD", ".BAT"].filter((ext) => upper.includes(ext));
	return [...preferred, ...upper.filter((ext) => !preferred.includes(ext))];
}

/** Ordered Windows probe names (.exe first, then .cmd/.bat); never the bare name. */
export function windowsExecutableCandidates(name: string, env: NodeJS.ProcessEnv = process.env): string[] {
	const candidates: string[] = [];
	for (const ext of windowsExtensions(env)) {
		for (const candidate of [`${name}${ext.toLowerCase()}`, `${name}${ext}`]) {
			if (!candidates.includes(candidate)) candidates.push(candidate);
		}
	}
	return candidates;
}

/**
 * Resolve a configured executable (bare name or path) to a spawnable file.
 * Explicit paths win as configured; bare names are scanned along PATH. On
 * Windows an extension-less hit is never selected from a scan, because npm
 * puts an unspawnable sh script at exactly that name.
 */
export function resolveExecutable(configured: string, deps: ExecutableResolverDeps = {}): ResolvedExecutable {
	const platform = deps.platform ?? process.platform;
	const env = deps.env ?? process.env;
	const statSync = deps.statSync ?? fs.statSync;
	const realpathSync = deps.realpathSync ?? ((target: string) => fs.realpathSync(target));
	const resolved = (file: string): ResolvedExecutable => ({ path: file, needsShell: executableNeedsShell(file, platform) });
	const isFile = (file: string): boolean => { try { return statSync(file).isFile(); } catch { return false; } };

	if (path.isAbsolute(configured) || configured.includes(path.sep) || (platform === "win32" && configured.includes("/"))) {
		const absolute = path.resolve(configured);
		if (platform !== "win32") return resolved(absolute);
		if (path.extname(absolute) && isFile(absolute)) return resolved(absolute);
		for (const candidate of windowsExecutableCandidates(absolute, env)) if (isFile(candidate)) return resolved(candidate);
		return resolved(absolute);
	}

	const names = platform === "win32"
		? [...(path.extname(configured) ? [configured] : []), ...windowsExecutableCandidates(configured, env)]
		: [configured];
	for (const dir of (env.PATH ?? "").split(path.delimiter)) {
		for (const name of names) {
			const candidate = path.join(dir, name);
			try { if (statSync(candidate).isFile()) return resolved(realpathSync(candidate)); } catch {}
		}
	}
	return resolved(configured);
}

// cmd.exe re-parses the whole command line when running a .cmd/.bat file, and
// npm cmd shims expand %* through cmd once more. Escaping model from
// cross-spawn (MIT): quote each argument, caret-escape cmd metacharacters,
// and escape twice for the shim's second parse.
const CMD_META_CHARS = /([()\][%!^"`<>&|;, *?])/g;

function escapeCmdArgument(argument: string): string {
	let escaped = argument.replace(/(\\*)"/g, '$1$1\\"');
	escaped = escaped.replace(/(\\*)$/, "$1$1");
	escaped = `"${escaped}"`;
	escaped = escaped.replace(CMD_META_CHARS, "^$1");
	return escaped.replace(CMD_META_CHARS, "^$1");
}

/**
 * Prepare a command for `spawn(..., { shell: false })`. Windows .cmd/.bat
 * files are not real executables, so they are routed through
 * `cmd.exe /d /s /c` with every argument quoted and caret-escaped — arguments
 * can carry user text such as prompts, so a bare `shell: true` (which joins
 * arguments unescaped) would be a command-injection hole.
 */
export function toSpawnableCommand(command: string, args: readonly string[], deps: Pick<ExecutableResolverDeps, "platform" | "env"> = {}): SpawnableCommand {
	const platform = deps.platform ?? process.platform;
	if (!executableNeedsShell(command, platform)) return { command, args: [...args] };
	const env = deps.env ?? process.env;
	const commandLine = [path.normalize(command).replace(CMD_META_CHARS, "^$1"), ...args.map(escapeCmdArgument)].join(" ");
	return { command: env.comspec ?? "cmd.exe", args: ["/d", "/s", "/c", `"${commandLine}"`], windowsVerbatimArguments: true };
}
