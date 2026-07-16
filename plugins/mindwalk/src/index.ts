import type {
  ExtensionAPI,
  ExtensionCommandContext,
  ExtensionContext,
} from "@earendil-works/pi-coding-agent";
import { assertMindwalkBinary, resolveMindwalkBinary } from "./binary.ts";
import { MindwalkProcessManager } from "./process-manager.ts";

export const MINDWALK_COMMAND = "mindwalk";

type MindwalkManager = Pick<MindwalkProcessManager, "open" | "stop">;

export interface MindwalkExtensionDependencies {
  createManager?: (openBrowser: (url: string) => Promise<boolean>) => MindwalkManager;
}

export default function mindwalkExtension(pi: ExtensionAPI): void {
  registerMindwalk(pi);
}

export function registerMindwalk(
  pi: ExtensionAPI,
  dependencies: MindwalkExtensionDependencies = {},
): void {
  let manager: MindwalkManager | undefined;

  const notify = (
    ctx: Pick<ExtensionContext, "ui">,
    message: string,
    type: "info" | "warning" | "error",
  ) => {
    ctx.ui.notify(message, type);
  };

  const openBrowser = async (url: string): Promise<boolean> => {
    const command = browserCommand(url);
    try {
      const result = await pi.exec(command.file, command.args, { timeout: 5_000 });
      return result.code === 0;
    } catch {
      return false;
    }
  };

  const getManager = (): MindwalkManager => {
    if (manager) return manager;
    if (dependencies.createManager) {
      manager = dependencies.createManager(openBrowser);
      return manager;
    }
    const binaryPath = resolveMindwalkBinary();
    assertMindwalkBinary(binaryPath);
    manager = new MindwalkProcessManager({ binaryPath, openBrowser });
    return manager;
  };

  pi.registerCommand(MINDWALK_COMMAND, {
    description: "Open this Pi session as a Mindwalk codebase replay",
    handler: async (args: string, ctx: ExtensionCommandContext) => {
      if (args.trim()) {
        notify(ctx, "Usage: /mindwalk", "error");
        return;
      }
      const sessionFile = ctx.sessionManager.getSessionFile();
      if (!sessionFile) {
        notify(ctx, "Mindwalk needs a persisted Pi session.", "warning");
        return;
      }

      try {
        const result = await getManager().open(sessionFile, ctx.cwd);
        if (!result.browserOpened) {
          notify(ctx, `Mindwalk is ready at ${result.url}`, "warning");
          return;
        }
        notify(
          ctx,
          result.reused
            ? `Reopened Mindwalk at ${result.url}`
            : `Mindwalk is ready at ${result.url}`,
          "info",
        );
      } catch (error) {
        notify(ctx, error instanceof Error ? error.message : String(error), "error");
      }
    },
  });

  pi.on("session_shutdown", async () => {
    await manager?.stop();
    manager = undefined;
  });
}

export function browserCommand(
  url: string,
  platform: NodeJS.Platform = process.platform,
): { file: string; args: string[] } {
  switch (platform) {
    case "darwin":
      return { file: "open", args: [url] };
    case "linux":
      return { file: "xdg-open", args: [url] };
    case "win32":
      return { file: "rundll32", args: ["url.dll,FileProtocolHandler", url] };
    default:
      throw new Error(`Cannot open a browser on ${platform}`);
  }
}
