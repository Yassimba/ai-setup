import { accessSync, constants } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const PLATFORM_NAMES: Partial<Record<NodeJS.Platform, string>> = {
  darwin: "darwin",
  linux: "linux",
  win32: "windows",
};

const ARCH_NAMES: Partial<Record<NodeJS.Architecture, string>> = {
  arm64: "arm64",
  x64: "amd64",
};

export interface BinaryResolutionOptions {
  platform?: NodeJS.Platform;
  arch?: NodeJS.Architecture;
  packageRoot?: string;
  override?: string;
}

export function mindwalkPackageRoot(): string {
  return resolve(dirname(fileURLToPath(import.meta.url)), "..");
}

export function resolveMindwalkBinary(options: BinaryResolutionOptions = {}): string {
  const override = options.override ?? process.env.MINDWALK_BIN;
  if (override) return resolve(override);

  const platform = options.platform ?? process.platform;
  const arch = options.arch ?? process.arch;
  const osName = PLATFORM_NAMES[platform];
  const archName = ARCH_NAMES[arch];
  if (!osName || !archName) {
    throw new Error(`Mindwalk does not support ${platform}/${arch}`);
  }

  const executable = platform === "win32" ? "mindwalk.exe" : "mindwalk";
  return join(
    options.packageRoot ?? mindwalkPackageRoot(),
    "dist",
    `${osName}-${archName}`,
    executable,
  );
}

export function assertMindwalkBinary(path: string, platform = process.platform): void {
  try {
    accessSync(path, platform === "win32" ? constants.F_OK : constants.X_OK);
  } catch {
    throw new Error(
      `Mindwalk binary is missing or not executable at ${path}. Reinstall @yassimba/pi-mindwalk.`,
    );
  }
}
