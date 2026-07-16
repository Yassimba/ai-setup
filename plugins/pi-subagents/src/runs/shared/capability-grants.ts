import { createHash } from "node:crypto";
import * as fs from "node:fs";
import * as path from "node:path";

export function resolveCanonicalExecutable(executable: string): string {
	return fs.realpathSync(path.resolve(executable)).toString();
}

export function sha256(value: string | Uint8Array): string {
	return createHash("sha256").update(value).digest("hex");
}

export type CapabilityRequest = {
	role: string;
	roleContent: string;
	workflow: string;
	workflowContent: string;
	backend: string;
	executable: string;
};

export type CapabilityGrant = {
	role: string;
	workflow: string;
	backend: string;
	roleDigest: string;
	workflowDigest: string;
	executable: string;
	/** Digest of the canonical real path. In-place CLI upgrades intentionally retain approval. */
	executableDigest: string;
	grantedAt: number;
};

type GrantFs = Pick<typeof fs, "existsSync" | "mkdirSync" | "readFileSync" | "writeFileSync" | "renameSync" | "rmSync" | "realpathSync">;
type GrantPath = Pick<typeof path, "dirname" | "join" | "resolve">;

type GrantStoreOptions = {
	agentDir: string;
	fs?: GrantFs;
	path?: GrantPath;
	now?: () => number;
};

function sameGrant(a: CapabilityGrant, b: CapabilityGrant): boolean {
	return a.role === b.role && a.workflow === b.workflow && a.backend === b.backend
		&& a.roleDigest === b.roleDigest && a.workflowDigest === b.workflowDigest
		&& a.executable === b.executable && a.executableDigest === b.executableDigest;
}

export function createCapabilityGrantStore(options: GrantStoreOptions) {
	const fsImpl = options.fs ?? fs;
	const pathImpl = options.path ?? path;
	const now = options.now ?? Date.now;
	const filePath = pathImpl.join(options.agentDir, "capability-grants.json");
	const resolve = (request: CapabilityRequest): CapabilityGrant => {
		const executable = fsImpl.realpathSync(pathImpl.resolve(request.executable)).toString();
		return {
			role: request.role,
			workflow: request.workflow,
			backend: request.backend,
			roleDigest: sha256(request.roleContent),
			workflowDigest: sha256(request.workflowContent),
			executable,
			executableDigest: sha256(executable),
			grantedAt: now(),
		};
	};
	const read = (): CapabilityGrant[] => {
		if (!fsImpl.existsSync(filePath)) return [];
		try {
			const parsed = JSON.parse(fsImpl.readFileSync(filePath, "utf8") as string);
			return Array.isArray(parsed?.grants) ? parsed.grants : [];
		} catch {
			return [];
		}
	};
	const write = (grants: CapabilityGrant[]): void => {
		fsImpl.mkdirSync(pathImpl.dirname(filePath), { recursive: true });
		const temp = `${filePath}.${now()}.tmp`;
		try {
			fsImpl.writeFileSync(temp, `${JSON.stringify({ version: 1, grants }, null, 2)}\n`, "utf8");
			fsImpl.renameSync(temp, filePath);
		} finally {
			fsImpl.rmSync(temp, { force: true });
		}
	};
	return {
		filePath,
		grant(request: CapabilityRequest): CapabilityGrant {
			const candidate = resolve(request);
			const grants = read().filter((grant) => !sameGrant(grant, candidate));
			grants.push(candidate);
			grants.sort((a, b) => [a.role, a.workflow, a.backend, a.executable].join("\0").localeCompare([b.role, b.workflow, b.backend, b.executable].join("\0")));
			write(grants);
			return candidate;
		},
		verify(request: CapabilityRequest): { allowed: boolean; reason?: string } {
			let candidate: CapabilityGrant;
			try { candidate = resolve(request); } catch { return { allowed: false, reason: "executable-unavailable" }; }
			return read().some((grant) => sameGrant(grant, candidate)) ? { allowed: true } : { allowed: false, reason: "no-exact-grant" };
		},
	};
}
