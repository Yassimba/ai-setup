import type {
  BeforeProviderRequestEvent,
  ExtensionAPI,
  ExtensionCommandContext,
  ExtensionContext,
} from "@earendil-works/pi-coding-agent";
import { defaultFastConfig, type FastConfig, loadConfig, saveDesiredActive } from "./config.ts";
import { FastFooter, type FooterContext, type FooterModel, type FooterTheme } from "./footer.ts";

export const FAST_EXTENSION_CAPABILITIES = ["fast-mode", "footer-status-feedback"] as const;
export type FastExtensionCapability = (typeof FAST_EXTENSION_CAPABILITIES)[number];

export const FAST_COMMAND = "fast";
export const FAST_FLAG = "fast";
export const FAST_STATUS_KEY = "pi-openai-fast";
export const FAST_DESIRED_HANDOFF_ENV = "PI_OPENAI_FAST_DESIRED";
export const PRIORITY_SERVICE_TIER = "priority";

export const FAST_REQUESTED_INACTIVE_NO_MODEL_WARNING =
  "Fast Mode is requested but inactive because no model is selected.";
export const FAST_REQUESTED_INACTIVE_UNSUPPORTED_MODEL_WARNING =
  "Fast Mode is requested but inactive because the current model is not supported.";

function modelKey(model: unknown): string | undefined {
  if (typeof model !== "object" || model === null) return undefined;
  const { provider, id } = model as { provider?: unknown; id?: unknown };
  return typeof provider === "string" && typeof id === "string" ? `${provider}/${id}` : undefined;
}

/** Toggling /fast exports the preference so newly launched subagents inherit it. */
function writeFastDesiredHandoff(desiredActive: boolean): void {
  process.env[FAST_DESIRED_HANDOFF_ENV] = desiredActive ? "1" : "0";
}

function readFastDesiredHandoff(): { desiredActive?: boolean; warning?: string } {
  const value = process.env[FAST_DESIRED_HANDOFF_ENV];
  if (value === undefined) return {};
  if (value === "1" || value === "0") return { desiredActive: value === "1" };
  return {
    warning: `Ignoring invalid ${FAST_DESIRED_HANDOFF_ENV} value ${JSON.stringify(value)}; expected exact value 1 or 0.`,
  };
}

export default function piOpenAIFast(pi: ExtensionAPI): void {
  registerPiOpenAIFast(pi);
}

export function registerPiOpenAIFast(pi: ExtensionAPI): void {
  let config: FastConfig = defaultFastConfig();
  let configLoad: Promise<{ warnings: string[] }> | undefined;
  let configLoaded = false;
  let desiredActive = false;
  let currentModel: FooterModel | undefined;
  let installedFooter: FastFooter | undefined;
  let ownsStatus = false;
  let footerView: FooterContext | undefined;

  pi.registerFlag(FAST_FLAG, {
    description: "Start this session with Fast Mode enabled",
    type: "boolean",
    default: false,
  });

  function isActive(): boolean {
    const key = modelKey(currentModel);
    return desiredActive && key !== undefined && config.supportedModels.includes(key);
  }

  function inactiveReason(): "no-model" | "unsupported-model" | undefined {
    if (!desiredActive || isActive()) return undefined;
    return modelKey(currentModel) === undefined ? "no-model" : "unsupported-model";
  }

  function notify(
    ui: ExtensionContext["ui"] | undefined,
    message: string,
    type: "info" | "warning" | "error",
  ): void {
    try {
      ui?.notify?.(message, type);
    } catch {
      // Notification sinks must not make lifecycle or command handling fail.
    }
  }

  function deliverWarnings(
    warnings: readonly string[],
    ui: ExtensionContext["ui"] | undefined,
  ): void {
    for (const message of new Set(warnings)) {
      if (typeof ui?.notify === "function") notify(ui, message, "warning");
      else console.warn(`[pi-openai-fast] ${message}`);
    }
  }

  /** Apply a state change and notify when Fast Mode newly becomes requested-but-inactive. */
  function transition(
    update: { desiredActive?: boolean; model?: FooterModel | undefined },
    ui: ExtensionContext["ui"] | undefined,
  ): void {
    const wasRequestedInactive = inactiveReason() !== undefined;
    if (update.desiredActive !== undefined) desiredActive = update.desiredActive;
    if (Object.hasOwn(update, "model")) currentModel = update.model;
    const reason = inactiveReason();
    if (reason !== undefined && !wasRequestedInactive) {
      notify(
        ui,
        reason === "no-model"
          ? FAST_REQUESTED_INACTIVE_NO_MODEL_WARNING
          : FAST_REQUESTED_INACTIVE_UNSUPPORTED_MODEL_WARNING,
        "warning",
      );
    }
  }

  function syncFooter(ctx: ExtensionContext, model = ctx.model as FooterModel | undefined): void {
    footerView = {
      model,
      sessionManager: ctx.sessionManager,
      modelRegistry: ctx.modelRegistry,
      getContextUsage: () => ctx.getContextUsage(),
    };
    const ui = ctx.ui;
    const showStatus = config.footer.mode === "status" && isActive();
    if (typeof ui?.setStatus === "function") {
      ui.setStatus(FAST_STATUS_KEY, showStatus ? "fast" : undefined);
      ownsStatus = showStatus;
    }

    if (config.footer.mode !== "replace") {
      clearFooter(ctx);
      return;
    }
    if (installedFooter?.isOwnedByExtension()) {
      installedFooter.invalidate();
      return;
    }
    installedFooter = undefined;
    if (typeof ui?.setFooter !== "function") return;
    ui.setFooter((tui, theme, footerData) => {
      const footer = new FastFooter({
        getContext: () => footerView,
        footerData,
        theme: theme as FooterTheme,
        isFastActive: isActive,
        getThinkingLevel: () => pi.getThinkingLevel(),
        fastLabelColors: {
          dark: config.footer.darkFastColor,
          light: config.footer.lightFastColor,
          vars: { ...config.footer.vars },
        },
        tui,
      });
      installedFooter = footer;
      return footer;
    });
  }

  function clearFooter(ctx: ExtensionContext | undefined): void {
    if (!installedFooter) return;
    if (installedFooter.isOwnedByExtension()) {
      installedFooter.dispose();
      if (typeof ctx?.ui?.setFooter === "function") ctx.ui.setFooter(undefined);
    }
    installedFooter = undefined;
  }

  async function loadConfigOnce(cwd: string): Promise<string[]> {
    if (configLoaded) return [];
    configLoad ??= loadConfig(cwd).then((result) => {
      config = result.config;
      configLoaded = true;
      return result;
    });
    return (await configLoad).warnings;
  }

  /** Session startup: load config once, then apply flag > env handoff > persisted preference. */
  async function startSession(ctx: ExtensionContext, model = ctx.model as FooterModel | undefined) {
    const warnings = [...(await loadConfigOnce(ctx.cwd))];
    const handoff = readFastDesiredHandoff();
    if (handoff.warning !== undefined) warnings.push(handoff.warning);
    deliverWarnings(warnings, ctx.ui);
    const startupFastOverride = pi.getFlag(FAST_FLAG) === true;
    if (startupFastOverride) writeFastDesiredHandoff(true);
    const desired = startupFastOverride
      ? true
      : (handoff.desiredActive ?? (config.persistState ? config.desiredActive : false));
    transition({ desiredActive: desired, model }, ctx.ui);
    syncFooter(ctx, model);
  }

  pi.registerCommand(FAST_COMMAND, {
    description: "Toggle Fast Mode priority service tier",
    handler: async (args: string, ctx: ExtensionCommandContext) => {
      if (args.trim().length > 0) {
        notify(ctx.ui, "Usage: /fast", "error");
        return;
      }
      if (!configLoaded) await startSession(ctx);
      transition(
        { desiredActive: !desiredActive, model: ctx.model as FooterModel | undefined },
        ctx.ui,
      );
      writeFastDesiredHandoff(desiredActive);
      config = { ...config, desiredActive };
      if (config.persistState) {
        const saved = await saveDesiredActive(ctx.cwd, desiredActive);
        deliverWarnings(saved.warnings, ctx.ui);
      }
      syncFooter(ctx);
    },
  });

  pi.on("session_start", async (_event, ctx) => {
    await startSession(ctx);
  });

  pi.on("session_shutdown", async (_event, ctx: ExtensionContext) => {
    clearFooter(ctx);
    if (ownsStatus && typeof ctx.ui?.setStatus === "function") {
      ctx.ui.setStatus(FAST_STATUS_KEY, undefined);
    }
    ownsStatus = false;
  });

  pi.on("model_select", async (event, ctx: ExtensionContext) => {
    const model = event.model as FooterModel | undefined;
    if (!configLoaded) {
      await startSession(ctx, model);
      return;
    }
    transition({ model }, ctx.ui);
    syncFooter(ctx, model);
  });

  pi.on("thinking_level_select", (_event, ctx: ExtensionContext) => {
    if (configLoaded) syncFooter(ctx);
  });

  pi.on("before_provider_request", (event: BeforeProviderRequestEvent) => {
    const payload = event.payload;
    if (typeof payload !== "object" || payload === null || Array.isArray(payload)) {
      return undefined;
    }
    const prototype = Object.getPrototypeOf(payload);
    if ((prototype !== Object.prototype && prototype !== null) || !isActive()) {
      return undefined;
    }
    return { ...(payload as Record<string, unknown>), service_tier: PRIORITY_SERVICE_TIER };
  });
}
