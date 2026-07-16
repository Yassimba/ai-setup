/**
 * reviewr collaboration extension for Pi.
 *
 * Binds this Pi session to the reviewr pane reviewing the same worktree (or the Deep Review
 * target pinned via `REVIEWR_COLLAB_SOCKET`/`REVIEWR_COLLAB_TARGET`):
 *
 * - captures the review context (target, location, selection patch, selected discussion,
 *   context tray) at prompt submission time and injects it into the turn, so later pane
 *   navigation can never retarget an already-submitted question;
 * - reports read/search/edit tool locations and completed edits so reviewr can follow;
 * - exposes `stage_review_draft`, letting the model stage local, unpublished review drafts
 *   that reviewr owns from then on. The channel has no publish operation of any kind.
 *
 * Pi stays fully usable when reviewr is away: prompts are marked as lacking review context,
 * tool reporting goes quiet, and draft staging fails visibly instead of queueing.
 */

import {
  defineTool,
  type ExtensionAPI,
  isEditToolResult,
  isToolCallEventType,
} from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { CollabClient, worktreeRoot } from "./client.ts";

interface ContextLocation {
  path?: string;
  side?: string;
  line?: number;
  start_line?: number | null;
}

interface ContextResource {
  alias?: string;
  kind?: string;
  author?: string;
  anchor?: string;
  body?: string;
  patch?: string | null;
  replies_complete?: boolean;
  replies?: { author?: string; body?: string }[];
  thread?: string | null;
}

interface ReviewContext {
  target?: string;
  source?: string;
  worktree?: string;
  location?: ContextLocation | null;
  patch?: string | null;
  item?: ContextResource | null;
  tray?: ContextResource[];
}

function indented(block: string, indent: string): string[] {
  return block.split("\n").map((row) => `${indent}${row}`);
}

function describeResource(resource: ContextResource, indent: string): string[] {
  const alias = resource.alias ? `${resource.alias} ` : "";
  const head = `${indent}${alias}${resource.kind ?? "item"} by @${resource.author ?? "?"} — ${resource.anchor ?? ""}`;
  const lines = [head];
  if (resource.thread) {
    lines.push(`${indent}  discussion id: ${resource.thread}`);
  }
  lines.push(...indented(resource.body ?? "", `${indent}  `));
  for (const reply of resource.replies ?? []) {
    lines.push(`${indent}  ↳ @${reply.author ?? "?"}: ${(reply.body ?? "").split("\n").join(" ")}`);
  }
  if (resource.replies_complete === false) {
    lines.push(`${indent}  (reply chain incomplete — more replies exist on the forge)`);
  }
  if (resource.patch) {
    lines.push(`${indent}  patch:`, ...indented(resource.patch, `${indent}    `));
  }
  return lines;
}

function locationLine(location: ContextLocation): string[] {
  if (!location.path) {
    return [];
  }
  const range =
    location.start_line != null && location.start_line !== location.line
      ? `${location.start_line}-${location.line}`
      : `${location.line}`;
  return [`location: ${location.path}:${range} (${location.side ?? "new"} side)`];
}

/** The snapshot as a readable context message for the model. */
export function renderContext(raw: unknown): string {
  const context = (raw ?? {}) as ReviewContext;
  const lines = ["[reviewr context]"];
  lines.push(`reviewing: ${context.target ?? "?"} (${context.source ?? "?"})`);
  if (context.worktree) {
    lines.push(`worktree: ${context.worktree}`);
  }
  lines.push(...locationLine(context.location ?? {}));
  if (context.item) {
    lines.push("selected item:", ...describeResource(context.item, "  "));
  }
  for (const [index, entry] of (context.tray ?? []).entries()) {
    if (index === 0) {
      lines.push("context tray:");
    }
    lines.push(...describeResource(entry, "  "));
  }
  if (context.patch) {
    lines.push("visible patch around the location:", ...indented(context.patch, "  "));
  }
  return lines.join("\n");
}

export default function register(pi: ExtensionAPI): void {
  const root = worktreeRoot(process.cwd());
  const state = {
    // Only mark prompts as context-less when collaboration was ever expected: a plain
    // standalone Pi should not narrate a reviewr it never had.
    everLinked: process.env.REVIEWR_COLLAB_SOCKET !== undefined,
    rejected: null as string | null,
    pendingContext: null as unknown,
    contextMissing: false,
    draftSeq: 0,
  };
  let statusUi: ((up: boolean) => void) | null = null;
  const client = new CollabClient(root, {
    onLink: (up) => {
      state.everLinked ||= up;
      statusUi?.(up);
    },
    onRejected: (reason) => {
      state.rejected = reason;
    },
  });
  client.connect();

  pi.on("session_start", (_event, ctx) => {
    const id = ctx.sessionManager.getSessionId();
    if (id) {
      client.setSession(id);
    }
    if (ctx.hasUI) {
      // Keep the footer live: the link usually comes up moments after session start.
      statusUi = (up) => ctx.ui.setStatus("reviewr", up ? "reviewr ✦" : "reviewr ✧");
      statusUi(client.isLinked());
    }
  });

  // Capture the review context at submission time — one atomic snapshot per prompt.
  pi.on("input", async (event) => {
    if (event.text.startsWith("/")) {
      return undefined; // commands are not prompts; nothing to contextualize
    }
    state.pendingContext = await client.requestContext(1_500);
    state.contextMissing = state.pendingContext == null && state.everLinked;
    return undefined;
  });

  pi.on("before_agent_start", (_event, ctx) => {
    if (ctx.hasUI) {
      ctx.ui.setStatus("reviewr", client.isLinked() ? "reviewr ✦" : "reviewr ✧");
    }
    const context = state.pendingContext;
    state.pendingContext = null;
    if (context != null) {
      return {
        message: {
          customType: "reviewr-context",
          content: renderContext(context),
          display: false,
          details: context,
        },
      };
    }
    if (state.contextMissing) {
      state.contextMissing = false;
      const why = state.rejected ? ` (${state.rejected})` : "";
      return {
        message: {
          customType: "reviewr-context",
          content: `[reviewr] This prompt carries no review context — reviewr is not connected${why}.`,
          display: false,
        },
      };
    }
    return undefined;
  });

  // Tool locations feed reviewr's follow mode; terminal-only activity carries no location.
  pi.on("tool_call", (event) => {
    if (isToolCallEventType("read", event)) {
      const offset = typeof event.input.offset === "number" ? event.input.offset : undefined;
      client.toolLocation("read", event.input.path, offset, event.toolCallId);
    } else if (isToolCallEventType("grep", event)) {
      if (typeof event.input.path === "string" && event.input.path.length > 0) {
        client.toolLocation("search", event.input.path, undefined, event.toolCallId);
      }
    } else if (isToolCallEventType("edit", event)) {
      client.toolLocation("edit", event.input.path, undefined, event.toolCallId);
    }
    return undefined;
  });

  pi.on("tool_result", (event) => {
    if (isEditToolResult(event) && !event.isError) {
      const file = typeof event.input.path === "string" ? event.input.path : "";
      if (file) {
        client.editCompleted(file, event.details?.firstChangedLine, event.toolCallId);
      }
    }
    return undefined;
  });

  pi.on("agent_start", () => {
    client.turnStarted();
  });

  pi.on("agent_settled", () => {
    client.turnSettled();
  });

  pi.on("session_shutdown", () => {
    client.shutdown();
  });

  pi.registerTool(
    defineTool({
      name: "stage_review_draft",
      label: "Stage review draft",
      description:
        "Stage a LOCAL, UNPUBLISHED review draft in the connected reviewr pane: either an " +
        "inline finding at a worktree location (path + line), or a reply to an existing " +
        "remote discussion (reply_to = the discussion id from the review context). Pass " +
        "draft_id to revise a draft you staged earlier. Drafts stay local until the human " +
        "reviewer explicitly syncs them; this tool cannot publish anything, and a draft the " +
        "reviewer has edited can no longer be revised — propose a new one instead.",
      promptSnippet:
        "stage_review_draft: stage a review finding or discussion reply as a local draft in reviewr",
      parameters: Type.Object({
        body: Type.String({ description: "The draft's text" }),
        path: Type.Optional(
          Type.String({ description: "Worktree-relative file path for an inline finding" }),
        ),
        line: Type.Optional(Type.Number({ description: "1-based line the finding ends at" })),
        start_line: Type.Optional(
          Type.Number({ description: "1-based first line of a ranged finding" }),
        ),
        reply_to: Type.Optional(
          Type.String({ description: "Remote discussion id to reply to (context `thread`)" }),
        ),
        draft_id: Type.Optional(
          Type.String({ description: "A draft id returned earlier, to revise that draft" }),
        ),
      }),
      async execute(_toolCallId, params) {
        const draft = params.draft_id ?? `pi-${process.pid}-${++state.draftSeq}`;
        await client.stageDraft({
          draft,
          body: params.body,
          path: params.path,
          line: params.line,
          start_line: params.start_line,
          reply_to: params.reply_to,
        });
        const where = params.reply_to
          ? `reply to discussion ${params.reply_to}`
          : `${params.path}:${params.line}`;
        return {
          content: [
            {
              type: "text" as const,
              text:
                `Draft ${draft} staged locally in reviewr (${where}). It stays local until ` +
                "the reviewer explicitly syncs it.",
            },
          ],
          details: { draft },
        };
      },
    }),
  );
}
