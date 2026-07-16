import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

type SnapshotFs = Pick<typeof fs, "chmodSync" | "existsSync" | "lstatSync" | "mkdirSync" | "mkdtempSync" | "readFileSync" | "rmSync" | "writeFileSync">;
type SnapshotOptions = {
	fs?: SnapshotFs;
	execFile?: typeof execFileSync;
	tempRoot?: string;
	maxFileBytes?: number;
};
export type ResearchSnapshot = { path: string; digest: string; files: string[]; excluded: string[]; cleanup: () => void };

const UNSAFE_NAME = /(?:^|\/)(?:\.git|node_modules)(?:\/|$)|(?:^|\/)(?:\.env(?:\..*)?|\.npmrc|\.netrc|\.pypirc|auth\.json|credentials?(?:\..*)?|service-account(?:\..*)?|id_(?:rsa|dsa|ecdsa|ed25519)|[^/]+\.(?:pem|p12|pfx|key))$/i;
const PRIVATE_KEY = /-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----/;
function isBinary(contents: Buffer): boolean {
	const sample = contents.subarray(0, 8192);
	return sample.includes(0);
}

export function createResearchSnapshot(repoPath: string, options: SnapshotOptions = {}): ResearchSnapshot {
	const fsImpl = options.fs ?? fs;
	const run = options.execFile ?? execFileSync;
	const maxBytes = options.maxFileBytes ?? 1024 * 1024;
	const repo = path.resolve(repoPath);
	const root = fsImpl.mkdtempSync(path.join(options.tempRoot ?? os.tmpdir(), "pi-research-snapshot-"));
	const raw = run("git", ["ls-files", "-z", "--cached", "--others", "--exclude-standard"], { cwd: repo, encoding: "buffer" }) as Buffer;
	const candidates = raw.toString("utf8").split("\0").filter(Boolean).sort((a, b) => a.localeCompare(b));
	const files: string[] = [];
	const excluded: string[] = [];
	const digest = createHash("sha256");
	for (const gitName of candidates) {
		const relative = gitName.split("\\").join("/");
		const source = path.resolve(repo, relative);
		if (UNSAFE_NAME.test(relative) || (!source.startsWith(`${repo}${path.sep}`) && source !== repo)) {
			excluded.push(relative);
			continue;
		}
		if (!fsImpl.existsSync(source)) continue;
		const stat = fsImpl.lstatSync(source);
		if (!stat.isFile() || stat.size > maxBytes) {
			excluded.push(relative);
			continue;
		}
		const contents = fsImpl.readFileSync(source) as Buffer;
		if (isBinary(contents) || PRIVATE_KEY.test(contents.toString("utf8"))) {
			excluded.push(relative);
			continue;
		}
		const target = path.join(root, ...relative.split("/"));
		fsImpl.mkdirSync(path.dirname(target), { recursive: true });
		fsImpl.writeFileSync(target, contents);
		fsImpl.chmodSync(target, 0o444);
		files.push(relative);
		digest.update(`${Buffer.byteLength(relative)}:${relative}:${contents.length}:`);
		digest.update(contents);
	}
	const immutableDirs = new Set<string>([root]);
	for (const relative of files) {
		let directory = path.dirname(path.join(root, ...relative.split("/")));
		while (directory.startsWith(root)) {
			immutableDirs.add(directory);
			if (directory === root) break;
			directory = path.dirname(directory);
		}
	}
	for (const directory of [...immutableDirs].sort((a, b) => b.length - a.length)) fsImpl.chmodSync(directory, 0o555);
	return {
		path: root,
		digest: digest.digest("hex"),
		files,
		excluded: excluded.sort((a, b) => a.localeCompare(b)),
		cleanup: () => {
			for (const directory of [...immutableDirs].sort((a, b) => a.length - b.length)) {
				try { fsImpl.chmodSync(directory, 0o755); } catch { /* already removed */ }
			}
			fsImpl.rmSync(root, { recursive: true, force: true });
		},
	};
}
