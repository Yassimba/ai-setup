import { createHash } from "node:crypto";
import * as path from "node:path";

type ContextSection = { kind: string; priority?: number; value: unknown };
export type ExternalContextInput = { workspace: string; sections: readonly ContextSection[]; hiddenMessages?: readonly unknown[] };
export type ExternalContextOptions = { mode?: "default" | "advisor-seed" | "advisor-delta"; allowedPaths?: readonly string[] };
export type ExternalContextResult = {
	text: string;
	bytes: number;
	hash: string;
	redactionCounts: Record<string, number>;
	incompleteMarkers: string[];
};

const CREDENTIAL_KEY = /(?:token|secret|password|passwd|api[_-]?key|authorization|cookie|credential)/i;
const PERSONAL_KEY = /(?:personalMemory|personal_memory|unrelatedMemory)/i;
const ENV_KEY = /^(?:env|environment)$/i;
const PRIVATE_KEY = /-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----/;
const INLINE_CREDENTIALS = [
	/\bBearer\s+[A-Za-z0-9._~+\/-]+=*/gi,
	/\b((?:api[_-]?key|access[_-]?token|auth[_-]?token|secret|password)\s*[:=]\s*)[^\s,;]+/gi,
	/\b(?:sk|rk|pk)[_-][A-Za-z0-9_-]{16,}\b/g,
] as const;
const UUID = /\b[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}\b/gi;
const TIMESTAMP = /\b\d{4}-\d\d-\d\d[T ][0-2]\d:[0-5]\d:[0-5]\d(?:\.\d+)?Z?\b/g;
const TEMP_PATH = /(?:\/tmp\/|\/var\/folders\/|[A-Za-z]:\\(?:Temp|Users\\[^\\]+\\AppData\\Local\\Temp)\\)[^\s"']+/gi;

export function canonicalizeExternalContext(input: ExternalContextInput, options: ExternalContextOptions = {}): ExternalContextResult {
	const counts: Record<string, number> = {};
	const incomplete = new Set<string>();
	const workspace = path.resolve(input.workspace);
	const allowedRoots = [workspace, ...(options.allowedPaths ?? []).map((item) => path.resolve(workspace, item))];
	const redact = (type: string): string => {
		counts[type] = (counts[type] ?? 0) + 1;
		return `<redacted:${type}>`;
	};
	const normalizeString = (raw: string): string => {
		let value = raw.replace(/\r\n?/g, "\n");
		if (PRIVATE_KEY.test(value)) return redact("private-key");
		for (const pattern of INLINE_CREDENTIALS) {
			value = value.replace(pattern, (_match, prefix?: string) => `${prefix ?? ""}${redact("credential")}`);
		}
		value = value.replace(UUID, () => redact("volatile-id"));
		value = value.replace(TIMESTAMP, () => redact("volatile-time"));
		value = value.replace(TEMP_PATH, () => redact("temporary-path"));
		if (path.isAbsolute(value) && !value.includes("\n")) {
			const absolute = path.resolve(value);
			const allowed = allowedRoots.some((root) => absolute === root || absolute.startsWith(`${root}${path.sep}`));
			if (!allowed) return redact("outside-path");
			return path.relative(workspace, absolute).split(path.sep).join("/") || ".";
		}
		return value;
	};
	const canonical = (value: unknown, key = ""): unknown => {
		if (CREDENTIAL_KEY.test(key)) return redact("credential");
		if (ENV_KEY.test(key)) return redact("environment");
		if (PERSONAL_KEY.test(key)) return redact("personal-memory");
		if (value === null || typeof value === "boolean" || typeof value === "number") return value;
		if (typeof value === "string") return normalizeString(value);
		if (Array.isArray(value)) {
			return value.map((item) => canonical(item));
		}
		if (typeof value === "object") {
			const result: Record<string, unknown> = {};
			for (const childKey of Object.keys(value as object).sort()) result[childKey] = canonical((value as Record<string, unknown>)[childKey], childKey);
			return result;
		}
		return `<unsupported:${typeof value}>`;
	};
	const sections = input.sections.map((section, index) => ({
		kind: section.kind,
		priority: section.priority ?? 100,
		index,
		value: canonical(section.value),
	}));
	if ((input.hiddenMessages?.length ?? 0) > 0) {
		sections.push({ kind: "hidden-orchestration", priority: 0, index: -1, value: redact("hidden-orchestration") });
	}
	sections.sort((a, b) => a.priority - b.priority || a.kind.localeCompare(b.kind) || a.index - b.index);
	const cap = options.mode === "advisor-seed" ? 200_000 : options.mode === "advisor-delta" ? 40_000 : Number.POSITIVE_INFINITY;
	let text = "";
	for (const section of sections) {
		const rendered = `## ${section.kind}\n${JSON.stringify(section.value, null, 2)}\n`;
		if (text.length + rendered.length <= cap) {
			text += rendered;
			continue;
		}
		incomplete.add("size-cap");
		const marker = "\n<incomplete:size-cap>\n";
		const room = Math.max(0, cap - text.length - marker.length);
		text += rendered.slice(0, room) + marker.slice(0, cap - text.length - room);
		break;
	}
	return {
		text,
		bytes: Buffer.byteLength(text),
		hash: createHash("sha256").update(text).digest("hex"),
		redactionCounts: Object.fromEntries(Object.entries(counts).sort(([a], [b]) => a.localeCompare(b))),
		incompleteMarkers: [...incomplete].sort(),
	};
}
