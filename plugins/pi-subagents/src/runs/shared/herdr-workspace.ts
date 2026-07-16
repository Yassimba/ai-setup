import { execFile } from "node:child_process";
import type { ExtensionConfig } from "../../shared/types.ts";

export interface HerdrWorkspaceSettings {
	workspaceLabel: string;
	keepPanes: boolean;
}

export interface HerdrPaneHandle {
	workspaceId: string;
	paneId: string;
	label: string;
}

interface HerdrCliResult { ok: boolean; value?: unknown; error?: string }
interface WorkspaceState {
	workspaceId: string;
	tabId?: string;
	stalePaneIds: string[];
	paneOrdinal: number;
	topPanes: Array<string | undefined>;
	bottomPanes: Array<string | undefined>;
	placementQueue: Promise<unknown>;
}

const DEFAULT_WORKSPACE_LABEL = "subagents";
const HERDR_CLI_TIMEOUT_MS = 10_000;
const workspaceLookups = new Map<string, Promise<WorkspaceState | undefined>>();

export function resolveHerdrWorkspaceSetting(value: ExtensionConfig["herdrWorkspace"]): HerdrWorkspaceSettings | undefined {
	if (value === true) return { workspaceLabel: DEFAULT_WORKSPACE_LABEL, keepPanes: false };
	if (!value || typeof value !== "object" || value.enabled === false) return undefined;
	return { workspaceLabel: value.workspaceLabel?.trim() || DEFAULT_WORKSPACE_LABEL, keepPanes: value.keepPanes === true };
}

function herdrCli(args: string[]): Promise<HerdrCliResult> {
	return new Promise((resolve) => {
		execFile(process.env.PI_SUBAGENTS_HERDR_EXECUTABLE ?? "herdr", args, { timeout: HERDR_CLI_TIMEOUT_MS }, (error, stdout) => {
			if (error && !stdout) return resolve({ ok: false, error: error.message });
			try {
				const parsed = JSON.parse(stdout) as { result?: unknown; error?: { message?: string } };
				if (parsed.error) return resolve({ ok: false, error: parsed.error.message ?? "herdr error" });
				resolve({ ok: true, value: parsed.result });
			} catch { resolve({ ok: false, error: `unparseable herdr output: ${stdout.slice(0, 120)}` }); }
		});
	});
}

async function paneIds(workspaceId: string): Promise<string[]> {
	const listed = await herdrCli(["pane", "list", "--workspace", workspaceId]);
	if (!listed.ok) return [];
	return ((listed.value as { panes?: Array<{ pane_id?: string }> })?.panes ?? []).map((p) => p.pane_id).filter((id): id is string => Boolean(id));
}
async function agentPaneIds(workspaceId: string): Promise<string[]> {
	const listed = await herdrCli(["agent", "list"]); if (!listed.ok) return [];
	return ((listed.value as { agents?: Array<{ pane_id?: string; workspace_id?: string }> })?.agents ?? []).filter((a) => a.workspace_id === workspaceId).map((a) => a.pane_id).filter((id): id is string => Boolean(id));
}

async function ensureWorkspace(label: string): Promise<WorkspaceState | undefined> {
	const existingLookup = workspaceLookups.get(label); if (existingLookup) return existingLookup;
	const lookup = (async () => {
		const listed = await herdrCli(["workspace", "list"]); if (!listed.ok) return undefined;
		const existing = ((listed.value as { workspaces?: Array<{ label?: string; workspace_id?: string }> })?.workspaces ?? []).find((w) => w.label === label);
		if (existing?.workspace_id) {
			const panes = await paneIds(existing.workspace_id);
			const agentPanes = await agentPaneIds(existing.workspace_id);
			const agents = new Set(agentPanes);
			const topPanes: Array<string | undefined> = [];
			const bottomPanes: Array<string | undefined> = [];
			for (const [index, paneId] of agentPanes.entries()) {
				const slot = gridSlotFor(index + 1);
				if (slot.row === "top") topPanes[slot.column] = paneId;
				else bottomPanes[slot.column] = paneId;
			}
			return {
				workspaceId: existing.workspace_id,
				stalePaneIds: panes.filter((id) => !agents.has(id)),
				paneOrdinal: agentPanes.length,
				topPanes,
				bottomPanes,
				placementQueue: Promise.resolve(),
			};
		}
		const created = await herdrCli(["workspace", "create", "--label", label, "--no-focus"]); if (!created.ok) return undefined;
		const workspaceId = (created.value as { workspace?: { workspace_id?: string } })?.workspace?.workspace_id; if (!workspaceId) return undefined;
		return { workspaceId, stalePaneIds: await paneIds(workspaceId), paneOrdinal: 0, topPanes: [], bottomPanes: [], placementQueue: Promise.resolve() };
	})();
	workspaceLookups.set(label, lookup); void lookup.then((state) => { if (!state) workspaceLookups.delete(label); }); return lookup;
}

export interface GridSlot { row: "top" | "bottom"; column: number }
export function gridSlotFor(n: number): GridSlot {
	if (n <= 2) return { row: "top", column: n - 1 };
	if (n <= 4) return { row: "bottom", column: n - 3 };
	return n % 2 === 1 ? { row: "top", column: (n - 1) / 2 } : { row: "bottom", column: (n - 2) / 2 };
}
async function bounce(workspace: WorkspaceState, paneId: string, direction: "right" | "down", anchor: string): Promise<void> {
	if (!workspace.tabId) return;
	if (!(await herdrCli(["pane", "move", paneId, "--new-tab", "--workspace", workspace.workspaceId, "--label", "placing", "--no-focus"])).ok) return;
	if (!(await herdrCli(["pane", "move", paneId, "--tab", workspace.tabId, "--split", direction, "--target-pane", anchor, "--no-focus"])).ok) await herdrCli(["pane", "move", paneId, "--tab", workspace.tabId, "--split", "right", "--no-focus"]);
}
function envFlags(env?: Record<string, string | undefined>): string[] { return Object.entries(env ?? {}).flatMap(([key, value]) => value === undefined ? [] : ["--env", `${key}=${value}`]); }

export class HerdrWorkspaceManager {
	readonly settings: HerdrWorkspaceSettings;

	constructor(settings: HerdrWorkspaceSettings) {
		this.settings = settings;
	}

	async startPane(input: { label: string; cwd: string; command: string; args: string[]; env?: Record<string, string | undefined> }): Promise<HerdrPaneHandle | undefined> {
		return this.startPaneWithRetry(input, true);
	}

	private async startPaneWithRetry(input: { label: string; cwd: string; command: string; args: string[]; env?: Record<string, string | undefined> }, retryStaleWorkspace: boolean): Promise<HerdrPaneHandle | undefined> {
		const workspace = await ensureWorkspace(this.settings.workspaceLabel);
		if (!workspace) return undefined;
		const workspaceLookup = workspaceLookups.get(this.settings.workspaceLabel);
		const place = async (): Promise<HerdrPaneHandle | undefined> => {
			const started = await herdrCli(["agent", "start", input.label, "--workspace", workspace.workspaceId, "--cwd", input.cwd, "--no-focus", ...envFlags(input.env), "--", input.command, ...input.args]);
			if (!started.ok) return undefined;
			const agent = (started.value as { agent?: { pane_id?: string; tab_id?: string } })?.agent;
			if (!agent?.pane_id) return undefined;
			workspace.tabId ??= agent.tab_id;
			for (const stale of workspace.stalePaneIds.splice(0)) await herdrCli(["pane", "close", stale]);
			const slot = gridSlotFor(++workspace.paneOrdinal);
			const anchor = slot.row === "top" ? workspace.topPanes[slot.column - 1] : workspace.topPanes[slot.column];
			if (anchor && anchor !== agent.pane_id) {
				await bounce(workspace, agent.pane_id, slot.row === "top" ? "right" : "down", anchor);
				if (slot.row === "top") {
					const bottom = workspace.bottomPanes[slot.column - 1];
					const top = workspace.topPanes[slot.column - 1];
					if (bottom && top) await bounce(workspace, bottom, "down", top);
				}
			}
			if (slot.row === "top") workspace.topPanes[slot.column] = agent.pane_id;
			else workspace.bottomPanes[slot.column] = agent.pane_id;
			return { workspaceId: workspace.workspaceId, paneId: agent.pane_id, label: input.label };
		};
		const queued = workspace.placementQueue.then(place, place);
		workspace.placementQueue = queued.catch(() => undefined);
		const pane = await queued;
		if (pane || !retryStaleWorkspace) return pane;
		// Herdr auto-closes an empty workspace. Evict a cached workspace that
		// disappeared after its last non-retained pane closed, then retry once.
		if (workspaceLookups.get(this.settings.workspaceLabel) === workspaceLookup) {
			workspaceLookups.delete(this.settings.workspaceLabel);
		}
		return this.startPaneWithRetry(input, false);
	}

	async isPaneAlive(handle: HerdrPaneHandle): Promise<boolean> {
		return (await herdrCli(["agent", "get", handle.label])).ok;
	}

	async interruptPane(handle: HerdrPaneHandle): Promise<void> {
		await herdrCli(["pane", "close", handle.paneId]);
	}

	async finishPane(handle: HerdrPaneHandle, outcome: "success" | "failure" | "cancelled"): Promise<void> {
		if (!this.settings.keepPanes || outcome === "cancelled") return this.interruptPane(handle);
		await herdrCli(["agent", "rename", handle.label, `${outcome === "success" ? "✓" : "✗"} ${handle.label}`]);
	}
}

export function resetHerdrWorkspaceManagerForTests(): void { workspaceLookups.clear(); }
