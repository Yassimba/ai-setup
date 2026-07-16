/**
 * The reviewr collaboration client: one newline-delimited-JSON connection to the reviewr
 * pane serving this worktree.
 *
 * Both sides derive the socket address independently — reviewr from its resolved repo root,
 * this extension from `git rev-parse --show-toplevel` — via the same FNV-1a hash, so no
 * discovery protocol is needed. `REVIEWR_COLLAB_SOCKET` / `REVIEWR_COLLAB_TARGET` override
 * both values when a Deep Review workspace pins the pair explicitly through the pane
 * environment. The client reconnects on a timer while reviewr is away, and goes permanently
 * quiet when reviewr rejects the hello (wrong protocol version or review target) — a stale
 * or unrelated Pi must never keep hammering another session's socket.
 */

import { execFileSync } from "node:child_process";
import { realpathSync } from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";

export const PROTOCOL_VERSION = 1;
const RECONNECT_MS = 3_000;

/** FNV-1a 64-bit over UTF-8 bytes, as a 16-digit hex string — must match reviewr's. */
export function fnv1a64(input: string): string {
  let hash = 0xcbf29ce484222325n;
  for (const byte of Buffer.from(input, "utf8")) {
    hash ^= BigInt(byte);
    hash = (hash * 0x100000001b3n) & 0xffffffffffffffffn;
  }
  return hash.toString(16).padStart(16, "0");
}

/** The worktree this Pi session lives in: the git toplevel, symlinks resolved. */
export function worktreeRoot(cwd: string): string {
  let top = cwd;
  try {
    top = execFileSync("git", ["rev-parse", "--show-toplevel"], {
      cwd,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
  } catch {
    // Not a repository: the cwd itself keys the (unlikely) session.
  }
  try {
    return realpathSync(top);
  } catch {
    return top;
  }
}

/**
 * The canonical identity of a worktree for hashing and target keys — reviewr applies the
 * identical normalization (context.rs `canonical_worktree_key`). On Windows, Rust's
 * canonicalize yields verbatim `\\?\C:\...` paths where realpathSync yields plain `C:\...`,
 * and NTFS ignores case — so the verbatim prefix is dropped and the path lowercased.
 * Identity only — never use the result for filesystem access.
 */
export function canonicalWorktreeKey(worktree: string): string {
  return process.platform === "win32" ? windowsKey(worktree) : worktree;
}

/** The Windows normalization of `canonicalWorktreeKey`, pure so every platform tests it. */
export function windowsKey(worktree: string): string {
  let plain = worktree;
  if (worktree.startsWith("\\\\?\\UNC\\")) {
    plain = `\\\\${worktree.slice("\\\\?\\UNC\\".length)}`;
  } else if (worktree.startsWith("\\\\?\\")) {
    plain = worktree.slice("\\\\?\\".length);
  }
  return plain.toLowerCase();
}

/** The deterministic socket address for a worktree — reviewr computes the identical value. */
export function socketPathFor(worktree: string): string {
  const user = process.env.USER ?? process.env.USERNAME ?? "anon";
  const hash = fnv1a64(`${user}|${canonicalWorktreeKey(worktree)}`);
  if (process.platform === "win32") {
    return `\\\\.\\pipe\\reviewr-collab-${hash}`;
  }
  return path.join(os.tmpdir(), `reviewr-collab-${hash}.sock`);
}

/** The default collaboration target key for a worktree — reviewr's `local:` form. */
export function localTargetFor(worktree: string): string {
  return `local:${canonicalWorktreeKey(worktree)}`;
}

interface PendingRequest {
  resolve: (value: unknown) => void;
  timer: NodeJS.Timeout;
}

export interface ClientEvents {
  /** The link came up (hello accepted) or went down. */
  onLink: (up: boolean) => void;
  /** reviewr rejected the hello; the client stays down for the rest of the process. */
  onRejected: (reason: string) => void;
}

/** See the module docs. */
export class CollabClient {
  readonly socketPath: string;
  readonly target: string;
  private socket: net.Socket | null = null;
  private buffer = "";
  private linked = false;
  private disabled: string | null = null;
  private piSession: string | null = null;
  private reconnect: NodeJS.Timeout | null = null;
  private nextRequest = 1;
  private readonly contexts = new Map<number, PendingRequest>();
  private readonly draftAcks = new Map<string, PendingRequest>();
  private readonly events: ClientEvents;

  constructor(worktree: string, events: ClientEvents) {
    this.socketPath = process.env.REVIEWR_COLLAB_SOCKET ?? socketPathFor(worktree);
    this.target = process.env.REVIEWR_COLLAB_TARGET ?? localTargetFor(worktree);
    this.events = events;
  }

  /** Whether a hello has been accepted on the live connection. */
  isLinked(): boolean {
    return this.linked;
  }

  /** Provide the Pi session id; (re)sends the hello once both socket and id exist. */
  setSession(sessionId: string): void {
    this.piSession = sessionId;
    this.hello();
  }

  /** Open the connection (and keep retrying quietly while reviewr is away). */
  connect(): void {
    if (this.socket || this.disabled) {
      return;
    }
    const socket = net.createConnection(this.socketPath);
    this.socket = socket;
    socket.setEncoding("utf8");
    socket.on("connect", () => this.hello());
    socket.on("data", (chunk: string) => this.onData(chunk));
    socket.on("error", () => {
      /* close follows; reconnect is scheduled there */
    });
    socket.on("close", () => {
      const wasLinked = this.linked;
      this.socket = null;
      this.linked = false;
      this.failAllPending("reviewr disconnected");
      if (wasLinked) {
        this.events.onLink(false);
      }
      this.scheduleReconnect();
    });
  }

  /** Send `bye` and stop reconnecting — the session is shutting down. */
  shutdown(): void {
    if (this.reconnect) {
      clearTimeout(this.reconnect);
      this.reconnect = null;
    }
    this.send({ v: PROTOCOL_VERSION, type: "bye" });
    this.socket?.end();
    this.socket = null;
    this.disabled = this.disabled ?? "shut down";
  }

  /** Request the atomic review-context snapshot for the prompt being submitted. */
  requestContext(timeoutMs: number): Promise<unknown | null> {
    if (!this.linked) {
      return Promise.resolve(null);
    }
    const request = this.nextRequest++;
    this.send({ v: PROTOCOL_VERSION, type: "prompt_context", request });
    return new Promise((resolve) => {
      const timer = setTimeout(() => {
        this.contexts.delete(request);
        resolve(null);
      }, timeoutMs);
      this.contexts.set(request, { resolve: (value) => resolve(value), timer });
    });
  }

  /** Report a tool location (read/search/edit) the agent is working at. */
  toolLocation(kind: "read" | "search" | "edit", file: string, line?: number, op?: string): void {
    if (!this.linked) {
      return;
    }
    this.send({ v: PROTOCOL_VERSION, type: "tool_location", kind, path: file, line, op });
  }

  /** Report one completed edit with its first changed line. */
  editCompleted(file: string, line?: number, op?: string): void {
    if (!this.linked) {
      return;
    }
    this.send({ v: PROTOCOL_VERSION, type: "edit_completed", path: file, line, op });
  }

  /** Report that an agent run started working. */
  turnStarted(): void {
    if (!this.linked) {
      return;
    }
    this.send({ v: PROTOCOL_VERSION, type: "turn_started" });
  }

  /** Report that the agent run fully settled. */
  turnSettled(): void {
    if (!this.linked) {
      return;
    }
    this.send({ v: PROTOCOL_VERSION, type: "turn_settled" });
  }

  /**
   * Stage (or revise) a local review draft and await reviewr's verdict. Rejects with the
   * named reason on a refused or timed-out stage — never queues an ambiguous mutation.
   */
  stageDraft(draft: {
    draft: string;
    body: string;
    path?: string;
    line?: number;
    start_line?: number;
    reply_to?: string;
  }): Promise<void> {
    if (!this.linked) {
      return Promise.reject(new Error("reviewr is not connected; the draft was not staged"));
    }
    this.send({ v: PROTOCOL_VERSION, type: "stage_draft", ...draft });
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.draftAcks.delete(draft.draft);
        reject(new Error("reviewr did not acknowledge the draft"));
      }, 3_000);
      this.draftAcks.set(draft.draft, {
        timer,
        resolve: (value) => {
          const ack = value as { ok?: boolean; reason?: string | null };
          if (ack.ok) {
            resolve();
          } else {
            reject(new Error(ack.reason ?? "reviewr refused the draft"));
          }
        },
      });
    });
  }

  private hello(): void {
    if (!this.socket || !this.piSession || this.disabled) {
      return;
    }
    this.send({
      v: PROTOCOL_VERSION,
      type: "hello",
      target: this.target,
      pi_session: this.piSession,
    });
  }

  private send(frame: Record<string, unknown>): void {
    this.socket?.write(`${JSON.stringify(frame)}\n`);
  }

  private scheduleReconnect(): void {
    if (this.reconnect || this.disabled) {
      return;
    }
    this.reconnect = setTimeout(() => {
      this.reconnect = null;
      this.connect();
    }, RECONNECT_MS);
  }

  private failAllPending(reason: string): void {
    for (const [, pending] of this.contexts) {
      clearTimeout(pending.timer);
      pending.resolve(null);
    }
    this.contexts.clear();
    for (const [, pending] of this.draftAcks) {
      clearTimeout(pending.timer);
      pending.resolve({ ok: false, reason });
    }
    this.draftAcks.clear();
  }

  private onData(chunk: string): void {
    this.buffer += chunk;
    let newline = this.buffer.indexOf("\n");
    while (newline >= 0) {
      const line = this.buffer.slice(0, newline);
      this.buffer = this.buffer.slice(newline + 1);
      this.onFrame(line);
      newline = this.buffer.indexOf("\n");
    }
  }

  private onFrame(line: string): void {
    let frame: Record<string, unknown>;
    try {
      frame = JSON.parse(line) as Record<string, unknown>;
    } catch {
      return;
    }
    switch (frame.type) {
      case "hello_ack": {
        if (frame.ok === true) {
          this.linked = true;
          this.events.onLink(true);
        } else {
          // A version or target mismatch is permanent for this process; retrying would
          // hammer a session that already refused us.
          this.disabled = typeof frame.reason === "string" ? frame.reason : "rejected";
          this.socket?.end();
          this.events.onRejected(this.disabled);
        }
        break;
      }
      case "context": {
        const request = typeof frame.request === "number" ? frame.request : -1;
        const pending = this.contexts.get(request);
        if (pending) {
          this.contexts.delete(request);
          clearTimeout(pending.timer);
          pending.resolve(frame.context ?? null);
        }
        break;
      }
      case "draft_ack": {
        const draft = typeof frame.draft === "string" ? frame.draft : "";
        const pending = this.draftAcks.get(draft);
        if (pending) {
          this.draftAcks.delete(draft);
          clearTimeout(pending.timer);
          pending.resolve({ ok: frame.ok === true, reason: frame.reason ?? null });
        }
        break;
      }
      default:
        break;
    }
  }
}
