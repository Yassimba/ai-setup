export type RunReceipt = {
	kind: "advisor" | "research" | "external";
	backend?: string;
	model?: string;
	effort?: string;
	resumed?: boolean;
	durationMs?: number;
	inputTokens?: number;
	outputTokens?: number;
	contextHash?: string;
	cacheReadTokens?: number;
	cacheHitRate?: number;
	cacheWriteTokens?: number;
	cacheOutputTokens?: number;
	retries?: number;
	degraded?: boolean;
	sourceCount?: number;
	reportPath?: string;
	snapshotHash?: string;
	executionMode?: "local" | "herdr" | "local-fallback";
	workspaceId?: string;
	paneId?: string;
};

const unknown = "—";
const integer = (value: number | undefined): string => value === undefined ? unknown : Math.round(value).toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",");
const duration = (value: number | undefined): string => value === undefined ? unknown : `${(value / 1000).toFixed(2)}s`;
const rate = (value: number | undefined): string => value === undefined ? unknown : `${(value * 100).toFixed(1)}%`;
const yesNo = (value: boolean | undefined): string => value === undefined ? unknown : value ? "yes" : "no";
const value = (item: string | undefined): string => item?.trim() || unknown;

export function formatRunReceipt(receipt: RunReceipt): string {
	const title = receipt.kind === "advisor" ? "Advisor receipt" : receipt.kind === "research" ? "Research child receipt" : "External child receipt";
	const lines = [
		title,
		`Backend: ${value(receipt.backend)}`,
		`Model: ${value(receipt.model)}`,
		`Effort: ${value(receipt.effort)}`,
		`Execution: ${value(receipt.executionMode)}`,
		`Herdr: ${receipt.workspaceId || receipt.paneId ? `${value(receipt.workspaceId)} / ${value(receipt.paneId)}` : unknown}`,
		`Session: ${receipt.resumed === undefined ? unknown : receipt.resumed ? "resumed" : "fresh"}`,
		`Duration: ${duration(receipt.durationMs)}`,
		`Tokens: ${integer(receipt.inputTokens)} in / ${integer(receipt.outputTokens)} out`,
		`Context hash: ${value(receipt.contextHash)}`,
		`Cache: ${integer(receipt.cacheReadTokens)} read / ${rate(receipt.cacheHitRate)} hit / ${integer(receipt.cacheWriteTokens)} write / ${integer(receipt.cacheOutputTokens)} output`,
		`Retries: ${integer(receipt.retries)}`,
		`Degraded: ${yesNo(receipt.degraded)}`,
	];
	if (receipt.kind === "research") {
		lines.push(`Sources: ${integer(receipt.sourceCount)}`, `Report: ${value(receipt.reportPath)}`);
	}
	if (receipt.kind !== "advisor") lines.push(`Snapshot hash: ${value(receipt.snapshotHash)}`);
	return lines.join("\n");
}

export type ParentModelMetadata = Pick<RunReceipt, "backend" | "model" | "effort" | "contextHash" | "degraded">;
export function compactParentMetadata(receipt: ParentModelMetadata): Partial<ParentModelMetadata> {
	const result: Partial<ParentModelMetadata> = {};
	for (const key of ["backend", "model", "effort", "contextHash", "degraded"] as const) {
		if (receipt[key] !== undefined) Object.assign(result, { [key]: receipt[key] });
	}
	return result;
}
