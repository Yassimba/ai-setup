import { mkdir, readFile, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, join } from "node:path";

export const DEFAULT_SUPPORTED_MODELS = [
  "openai/gpt-5.4",
  "openai/gpt-5.5",
  "openai-codex/gpt-5.4",
  "openai-codex/gpt-5.5",
  "openai-codex/gpt-5.6-sol",
  "openai-codex/gpt-5.6-terra",
  "openai-codex/gpt-5.6-luna",
] as const;

export type FooterMode = "replace" | "status" | "off";
export type FastColorValue = string | number;

export interface FastFooterConfig {
  mode: FooterMode;
  vars: Record<string, string>;
  darkFastColor?: FastColorValue | undefined;
  lightFastColor?: FastColorValue | undefined;
}

export interface FastConfig {
  persistState: boolean;
  desiredActive: boolean;
  supportedModels: string[];
  footer: FastFooterConfig;
}

export const DEFAULT_FAST_CONFIG: FastConfig = {
  persistState: false,
  desiredActive: false,
  supportedModels: [...DEFAULT_SUPPORTED_MODELS],
  footer: { mode: "replace", vars: {} },
};

export function defaultFastConfig(): FastConfig {
  return {
    ...DEFAULT_FAST_CONFIG,
    supportedModels: [...DEFAULT_FAST_CONFIG.supportedModels],
    footer: { ...DEFAULT_FAST_CONFIG.footer, vars: {} },
  };
}

export interface FastConfigResult {
  config: FastConfig;
  warnings: string[];
}

// ---------------------------------------------------------------------------
// Color tokens: hex ("#rrggbb"), 256-color index (number or numeric string),
// variable names resolving through footer.vars, or "" for terminal default.

const HEX_COLOR = /^#[0-9a-fA-F]{6}$/;
const INTEGER_INDEX = /^\d+$/;
const COLOR_VAR = /^[A-Za-z_][A-Za-z0-9_.-]*$/;
// Literals written by pre-fork releases; ignored so upgrades fall back to theme-matched.
const LEGACY_COLOR_LITERALS = new Set(["#ff50be", "#d20000"]);

export function normalizeFastColorValue(value: unknown): FastColorValue | undefined {
  if (typeof value === "number") {
    return Number.isInteger(value) && value >= 0 && value <= 255 ? value : undefined;
  }
  if (typeof value !== "string") return undefined;
  const trimmed = value.trim();
  if (trimmed === "" || HEX_COLOR.test(trimmed)) return trimmed;
  if (INTEGER_INDEX.test(trimmed)) {
    return Number(trimmed) <= 255 ? trimmed : undefined;
  }
  return COLOR_VAR.test(trimmed) ? trimmed : undefined;
}

/** Resolve variable references to a concrete color token, or explain why not. */
export function resolveFastColorValue(
  value: FastColorValue,
  vars: Readonly<Record<string, string>>,
  visited = new Set<string>(),
): { value: FastColorValue } | { error: string } {
  if (
    typeof value === "number" ||
    value === "" ||
    HEX_COLOR.test(value) ||
    INTEGER_INDEX.test(value)
  ) {
    return { value };
  }
  if (visited.has(value)) {
    return { error: `variable ${JSON.stringify(value)} resolves circularly` };
  }
  visited.add(value);
  const referenced = Object.hasOwn(vars, value) ? vars[value] : undefined;
  if (typeof referenced !== "string") {
    return { error: `variable ${JSON.stringify(value)} is not defined` };
  }
  const normalized = normalizeFastColorValue(referenced);
  if (normalized === undefined) {
    return { error: "it is not a supported color token" };
  }
  return resolveFastColorValue(normalized, vars, visited);
}

// ---------------------------------------------------------------------------
// Field validation shared by load-merge and write-sanitize.

type JsonRecord = Record<string, unknown>;

function isRecord(value: unknown): value is JsonRecord {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isFooterMode(value: unknown): value is FooterMode {
  return value === "replace" || value === "status" || value === "off";
}

function describe(value: unknown): string {
  return JSON.stringify(value) ?? String(value);
}

function validModelKey(value: unknown): string | undefined {
  if (typeof value !== "string") return undefined;
  const trimmed = value.trim();
  const slash = trimmed.indexOf("/");
  const invalid =
    trimmed.length === 0 ||
    /\s/.test(trimmed) ||
    slash <= 0 ||
    slash === trimmed.length - 1 ||
    /[*[\](){}?+|^$\\]/.test(trimmed);
  return invalid ? undefined : trimmed;
}

/** Returns the valid entries, or undefined when the value is not an array at all. */
function readSupportedModels(
  value: unknown,
  path: string,
  warnings: string[],
): string[] | undefined {
  if (!Array.isArray(value)) {
    warnings.push(
      `Ignored supportedModels at ${path} because it must be an array of provider/model strings.`,
    );
    return undefined;
  }
  const kept: string[] = [];
  const dropped: unknown[] = [];
  for (const entry of value) {
    const normalized = validModelKey(entry);
    if (normalized === undefined) dropped.push(entry);
    else kept.push(normalized);
  }
  if (dropped.length > 0) {
    warnings.push(
      `Ignored invalid supportedModels entries at ${path}: ${dropped.map(describe).join(", ")}.`,
    );
  }
  if (value.length > 0 && kept.length === 0) {
    warnings.push(
      `All supportedModels entries at ${path} were invalid; Fast Mode has no supported models from that config layer.`,
    );
  }
  return kept;
}

function readColor(
  value: unknown,
  vars: Readonly<Record<string, string>>,
  field: string,
  path: string,
  warnings: string[],
): FastColorValue | undefined {
  if (typeof value === "string" && LEGACY_COLOR_LITERALS.has(value.trim().toLowerCase())) {
    return undefined;
  }
  const normalized = normalizeFastColorValue(value);
  const resolution =
    normalized === undefined
      ? { error: "it is not a supported color token" }
      : resolveFastColorValue(normalized, vars);
  if ("error" in resolution) {
    warnings.push(
      `Ignored invalid Fast label color ${field} at ${path}: ${describe(value)} (${resolution.error}).`,
    );
    return undefined;
  }
  return normalized;
}

function stringEntries(source: JsonRecord): Record<string, string> {
  return Object.fromEntries(
    Object.entries(source).filter(
      (entry): entry is [string, string] => typeof entry[1] === "string",
    ),
  );
}

function mergeConfig(
  base: FastConfig,
  source: JsonRecord,
  path: string,
  warnings: string[],
): FastConfig {
  const next: FastConfig = {
    ...base,
    supportedModels: [...base.supportedModels],
    footer: { ...base.footer, vars: { ...base.footer.vars } },
  };
  if (typeof source.persistState === "boolean") next.persistState = source.persistState;
  if (typeof source.desiredActive === "boolean") {
    next.desiredActive = source.desiredActive;
  } else if (!Object.hasOwn(source, "desiredActive") && typeof source.active === "boolean") {
    next.desiredActive = source.active; // legacy field name
  }
  if (Object.hasOwn(source, "supportedModels")) {
    const models = readSupportedModels(source.supportedModels, path, warnings);
    if (models !== undefined) next.supportedModels = models;
  }
  if (isRecord(source.footer)) mergeFooterConfig(next.footer, source.footer, path, warnings);
  return next;
}

function mergeFooterConfig(
  target: FastFooterConfig,
  source: JsonRecord,
  path: string,
  warnings: string[],
): void {
  if (isFooterMode(source.mode)) target.mode = source.mode;
  if (isRecord(source.vars)) target.vars = stringEntries(source.vars);
  for (const [key, field] of [
    ["darkFastColor", "footer.darkFastColor"],
    ["lightFastColor", "footer.lightFastColor"],
  ] as const) {
    if (Object.hasOwn(source, key)) {
      const color = readColor(source[key], target.vars, field, path, warnings);
      if (color !== undefined) target[key] = color;
    }
  }
}

function sanitizeFooterRecord(source: JsonRecord, path: string, warnings: string[]): JsonRecord {
  const footer: JsonRecord = { ...source };
  if (Object.hasOwn(footer, "mode") && !isFooterMode(footer.mode)) delete footer.mode;
  if (Object.hasOwn(footer, "vars")) {
    if (isRecord(footer.vars)) footer.vars = stringEntries(footer.vars);
    else delete footer.vars;
  }
  const vars = isRecord(footer.vars) ? stringEntries(footer.vars) : {};
  for (const [key, field] of [
    ["darkFastColor", "footer.darkFastColor"],
    ["lightFastColor", "footer.lightFastColor"],
  ] as const) {
    if (Object.hasOwn(footer, key)) {
      const color = readColor(footer[key], vars, field, path, warnings);
      if (color === undefined) delete footer[key];
      else footer[key] = color;
    }
  }
  return footer;
}

/** Sanitize a raw record before writing: keep unknown fields, drop invalid known ones. */
function sanitizeRecord(source: JsonRecord, path: string, warnings: string[]): JsonRecord {
  const next: JsonRecord = { ...source };
  delete next.active; // legacy field, superseded by desiredActive
  for (const field of ["persistState", "desiredActive"]) {
    if (Object.hasOwn(next, field) && typeof next[field] !== "boolean") delete next[field];
  }
  if (Object.hasOwn(next, "supportedModels")) {
    const models = readSupportedModels(next.supportedModels, path, warnings);
    if (models === undefined) delete next.supportedModels;
    else next.supportedModels = models;
  }
  if (Object.hasOwn(next, "footer")) {
    if (isRecord(next.footer)) next.footer = sanitizeFooterRecord(next.footer, path, warnings);
    else delete next.footer;
  }
  return next;
}

// ---------------------------------------------------------------------------
// File access.

export function configPaths(cwd: string): { project: string; global: string } {
  return {
    project: join(cwd, ".pi", "extensions", "pi-openai-fast.json"),
    global: join(homedir(), ".pi", "agent", "extensions", "pi-openai-fast.json"),
  };
}

type ReadResult = { kind: "missing" } | { kind: "failed" } | { kind: "loaded"; record: JsonRecord };

async function readRecord(path: string): Promise<ReadResult> {
  let text: string;
  try {
    text = await readFile(path, "utf8");
  } catch (error) {
    if (isRecord(error) && (error.code === "ENOENT" || error.code === "ENOTDIR")) {
      return { kind: "missing" };
    }
    return { kind: "failed" };
  }
  try {
    const parsed: unknown = JSON.parse(text);
    if (isRecord(parsed)) return { kind: "loaded", record: parsed };
  } catch {
    // fall through to failed
  }
  return { kind: "failed" };
}

async function writeRecord(path: string, record: JsonRecord, warnings: string[]): Promise<boolean> {
  try {
    await mkdir(dirname(path), { recursive: true });
    await writeFile(path, `${JSON.stringify(record, null, 2)}\n`, "utf8");
    return true;
  } catch {
    warnings.push(
      `Could not write pi-openai-fast config at ${path}; the config update was not saved.`,
    );
    return false;
  }
}

function configToRecord(config: FastConfig): JsonRecord {
  return {
    persistState: config.persistState,
    desiredActive: config.desiredActive,
    supportedModels: [...config.supportedModels],
    footer: { mode: config.footer.mode, vars: { ...config.footer.vars } },
  };
}

/** Load global then project config (project wins). Writes defaults when neither exists. */
export async function loadConfig(cwd: string): Promise<FastConfigResult> {
  const paths = configPaths(cwd);
  const warnings: string[] = [];
  let config = defaultFastConfig();

  const layers = await Promise.all([readRecord(paths.global), readRecord(paths.project)]);
  const [globalLayer, projectLayer] = layers;

  if (globalLayer.kind === "missing" && projectLayer.kind === "missing") {
    await writeRecord(paths.global, configToRecord(config), warnings);
    return { config, warnings };
  }
  for (const [layer, path] of [
    [globalLayer, paths.global],
    [projectLayer, paths.project],
  ] as const) {
    if (layer.kind === "failed") {
      warnings.push(
        `Could not read pi-openai-fast config at ${path}; using defaults for that config layer.`,
      );
    } else if (layer.kind === "loaded") {
      config = mergeConfig(config, layer.record, path, warnings);
    }
  }
  return { config, warnings };
}

/** Persist the desiredActive preference into the project config if present, else global. */
export async function saveDesiredActive(
  cwd: string,
  desiredActive: boolean,
): Promise<{ ok: boolean; warnings: string[] }> {
  const paths = configPaths(cwd);
  const warnings: string[] = [];
  const projectRead = await readRecord(paths.project);
  const target = projectRead.kind === "missing" ? paths.global : paths.project;
  const existing = projectRead.kind === "missing" ? await readRecord(paths.global) : projectRead;

  if (existing.kind === "failed") {
    warnings.push(
      `Could not save Fast Mode preference because the config at ${target} could not be read as a JSON object and needs manual repair before saving Fast Mode preferences.`,
    );
    return { ok: false, warnings };
  }
  const record =
    existing.kind === "loaded"
      ? sanitizeRecord(existing.record, target, warnings)
      : configToRecord(defaultFastConfig());
  record.desiredActive = desiredActive;
  const ok = await writeRecord(target, record, warnings);
  return { ok, warnings };
}
