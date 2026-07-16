import { type Component, truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import { type FastColorValue, resolveFastColorValue } from "./config.ts";

/*
 * The FastFooter render logic is adapted from Pi's default footer renderer:
 * @earendil-works/pi-coding-agent v0.75.3
 * packages/coding-agent/src/modes/interactive/components/footer.ts
 * pi-mono commit 144b93861f339ce353531f6873d377a1e4b2f5c4.
 *
 * Original project license: MIT License.
 * Copyright (c) 2025 Mario Zechner.
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in all
 * copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
 * SOFTWARE.
 */

export type ThinkingLevel = "off" | "minimal" | "low" | "medium" | "high" | "xhigh";
export type ColorMode = "truecolor" | "256color";

export interface FooterTheme {
  name?: string | undefined;
  fg(color: string, text: string): string;
  getColorMode?: () => ColorMode;
  getThinkingBorderColor?: (level: ThinkingLevel) => ((text: string) => string) | undefined;
}

export interface FooterModel {
  provider: string;
  id: string;
  reasoning?: boolean | undefined;
  contextWindow?: number | undefined;
}

interface SessionEntry {
  type: string;
  message?: {
    role?: string | undefined;
    usage?:
      | {
          input?: number | undefined;
          output?: number | undefined;
          cacheRead?: number | undefined;
          cacheWrite?: number | undefined;
          cost?: { total?: number | undefined } | undefined;
        }
      | undefined;
  };
}

export interface FooterContext {
  model?: FooterModel | undefined;
  sessionManager: {
    getCwd(): string;
    getSessionName(): string | undefined;
    getEntries(): readonly SessionEntry[];
  };
  modelRegistry: { isUsingOAuth(model: FooterModel): boolean };
  getContextUsage():
    | { percent?: number | null | undefined; contextWindow?: number | undefined }
    | undefined;
}

export interface FooterData {
  getGitBranch(): string | null | undefined;
  getExtensionStatuses(): ReadonlyMap<string, string>;
  getAvailableProviderCount(): number;
  onBranchChange?: (callback: () => void) => () => void;
}

export interface FastLabelColors {
  dark?: FastColorValue | undefined;
  light?: FastColorValue | undefined;
  vars: Readonly<Record<string, string>>;
}

// ---------------------------------------------------------------------------
// Color rendering: hex or 256-index token → foreground ANSI sequence.

const RESET_FG = "\x1b[39m";
const CUBE = [0, 95, 135, 175, 215, 255];

function nearestIndex(values: readonly number[], value: number): number {
  let best = 0;
  for (const [index, candidate] of values.entries()) {
    if (Math.abs(value - candidate) < Math.abs(value - values[best])) best = index;
  }
  return best;
}

function weightedDistance(r1: number, g1: number, b1: number, r2: number, g2: number, b2: number) {
  return (r1 - r2) ** 2 * 0.299 + (g1 - g2) ** 2 * 0.587 + (b1 - b2) ** 2 * 0.114;
}

function hexTo256(r: number, g: number, b: number): number {
  const ri = nearestIndex(CUBE, r);
  const gi = nearestIndex(CUBE, g);
  const bi = nearestIndex(CUBE, b);
  const cubeDistance = weightedDistance(r, g, b, CUBE[ri], CUBE[gi], CUBE[bi]);
  const gray = Math.round(0.299 * r + 0.587 * g + 0.114 * b);
  const grayIndex = nearestIndex(
    Array.from({ length: 24 }, (_, i) => 8 + i * 10),
    gray,
  );
  const grayValue = 8 + grayIndex * 10;
  const grayDistance = weightedDistance(r, g, b, grayValue, grayValue, grayValue);
  const isNearGray = Math.max(r, g, b) - Math.min(r, g, b) < 10;
  return isNearGray && grayDistance < cubeDistance ? 232 + grayIndex : 16 + 36 * ri + 6 * gi + bi;
}

export function fastColorToAnsi(color: FastColorValue, mode: ColorMode): string {
  if (color === "") return RESET_FG;
  if (typeof color === "number" || /^\d+$/.test(color)) return `\x1b[38;5;${Number(color)}m`;
  const r = Number.parseInt(color.slice(1, 3), 16);
  const g = Number.parseInt(color.slice(3, 5), 16);
  const b = Number.parseInt(color.slice(5, 7), 16);
  if ([r, g, b].some(Number.isNaN)) throw new Error(`Invalid color value: ${color}`);
  return mode === "truecolor" ? `\x1b[38;2;${r};${g};${b}m` : `\x1b[38;5;${hexTo256(r, g, b)}m`;
}

// ---------------------------------------------------------------------------
// The inline "fast" label.

export function normalizeThinkingLevel(value: string | undefined): ThinkingLevel {
  switch (value) {
    case "minimal":
    case "low":
    case "medium":
    case "high":
    case "xhigh":
      return value;
    default:
      return "off";
  }
}

function themeMatchedFastLabel(theme: FooterTheme, thinkingLevel: ThinkingLevel): string {
  if (thinkingLevel !== "off") {
    try {
      const render = theme.getThinkingBorderColor?.(thinkingLevel);
      if (typeof render === "function") return render("fast");
    } catch {
      // fall through to the dim label
    }
  }
  return theme.fg("dim", "fast");
}

export function renderFastLabel(
  theme: FooterTheme,
  thinkingLevel: ThinkingLevel,
  colors: FastLabelColors,
): string {
  const token = theme.name?.toLowerCase() === "light" ? colors.light : colors.dark;
  if (token !== undefined) {
    const resolution = resolveFastColorValue(token, colors.vars);
    if ("value" in resolution) {
      const mode = theme.getColorMode?.() === "truecolor" ? "truecolor" : "256color";
      return `${fastColorToAnsi(resolution.value, mode)}fast${RESET_FG}`;
    }
  }
  return themeMatchedFastLabel(theme, thinkingLevel);
}

// ---------------------------------------------------------------------------
// Footer component: pi's default footer with the fast label added to the
// model label. Owned by the extension while installed via ui.setFooter.

export interface FastFooterOptions {
  getContext: () => FooterContext | undefined;
  footerData: FooterData;
  theme: FooterTheme;
  isFastActive: () => boolean;
  getThinkingLevel: () => string | undefined;
  fastLabelColors?: FastLabelColors | undefined;
  tui?: { requestRender(force?: boolean): void } | undefined;
}

function formatTokens(count: number): string {
  if (count < 1000) return count.toString();
  if (count < 10000) return `${(count / 1000).toFixed(1)}k`;
  if (count < 1000000) return `${Math.round(count / 1000)}k`;
  if (count < 10000000) return `${(count / 1000000).toFixed(1)}M`;
  return `${Math.round(count / 1000000)}M`;
}

function cumulativeUsage(entries: readonly SessionEntry[]) {
  const total = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, cost: 0 };
  for (const entry of entries) {
    if (entry.type !== "message" || entry.message?.role !== "assistant") continue;
    const usage = entry.message.usage;
    total.input += usage?.input ?? 0;
    total.output += usage?.output ?? 0;
    total.cacheRead += usage?.cacheRead ?? 0;
    total.cacheWrite += usage?.cacheWrite ?? 0;
    total.cost += usage?.cost?.total ?? 0;
  }
  return total;
}

function formatContextUsage(context: FooterContext, theme: FooterTheme): string {
  const usage = context.getContextUsage();
  const contextWindow = usage?.contextWindow ?? context.model?.contextWindow ?? 0;
  const percentValue = usage?.percent ?? 0;
  const percent = usage?.percent === null ? "?" : `${percentValue.toFixed(1)}%`;
  const display = `${percent}/${formatTokens(contextWindow)} (auto)`;
  if (percentValue > 90) return theme.fg("error", display);
  if (percentValue > 70) return theme.fg("warning", display);
  return display;
}

function formatStatsLeft(context: FooterContext, theme: FooterTheme): string {
  const total = cumulativeUsage(context.sessionManager.getEntries());
  const parts: string[] = [];
  if (total.input) parts.push(`↑${formatTokens(total.input)}`);
  if (total.output) parts.push(`↓${formatTokens(total.output)}`);
  if (total.cacheRead) parts.push(`R${formatTokens(total.cacheRead)}`);
  if (total.cacheWrite) parts.push(`W${formatTokens(total.cacheWrite)}`);
  const model = context.model;
  const usingSubscription = model ? context.modelRegistry.isUsingOAuth(model) : false;
  if (total.cost || usingSubscription) {
    parts.push(`$${total.cost.toFixed(3)}${usingSubscription ? " (sub)" : ""}`);
  }
  parts.push(formatContextUsage(context, theme));
  return parts.join(" ");
}

function formatWorkingDirectory(context: FooterContext, footerData: FooterData): string {
  let directory = context.sessionManager.getCwd();
  const home = process.env.HOME || process.env.USERPROFILE;
  if (home && directory.startsWith(home)) directory = `~${directory.slice(home.length)}`;
  const branch = footerData.getGitBranch();
  if (branch) directory = `${directory} (${branch})`;
  const sessionName = context.sessionManager.getSessionName();
  return sessionName ? `${directory} • ${sessionName}` : directory;
}

// The stats line is dim-wrapped as a whole; carve the colored fast label out
// so its ANSI color survives the surrounding dim sequence.
const ESC = "\u001b";
const ANSI_FAST_LABEL = new RegExp(`${ESC}\\[[0-9;]*mfast${ESC}\\[39m`, "g");

function dimPreservingFastLabel(theme: FooterTheme, text: string): string {
  const matches = [...text.matchAll(ANSI_FAST_LABEL)];
  const last = matches.at(-1);
  if (!last) return theme.fg("dim", text);
  const before = text.slice(0, last.index);
  const after = text.slice(last.index + last[0].length);
  return [before ? theme.fg("dim", before) : "", last[0], after ? theme.fg("dim", after) : ""].join(
    "",
  );
}

export class FastFooter implements Component {
  private readonly options: FastFooterOptions;
  private readonly disposeCallbacks: Array<() => void> = [];
  private ownedByExtension = true;

  constructor(options: FastFooterOptions) {
    this.options = options;
    const unsubscribe = options.footerData.onBranchChange?.(() => this.invalidate());
    if (typeof unsubscribe === "function") this.disposeCallbacks.push(unsubscribe);
  }

  invalidate(): void {
    this.options.tui?.requestRender();
  }

  isOwnedByExtension(): boolean {
    return this.ownedByExtension;
  }

  dispose(): void {
    this.ownedByExtension = false;
    for (const dispose of this.disposeCallbacks) dispose();
    this.disposeCallbacks.length = 0;
  }

  render(width: number): string[] {
    const context = this.options.getContext();
    if (!context) return [];
    const renderWidth = Math.max(0, Math.floor(width));
    const { theme, footerData } = this.options;
    const model = context.model;
    const thinkingLevel = normalizeThinkingLevel(this.options.getThinkingLevel());

    let modelLabel = model?.id || "no-model";
    if (this.options.isFastActive()) {
      const colors = this.options.fastLabelColors ?? { vars: {} };
      modelLabel = `${modelLabel} ${renderFastLabel(theme, thinkingLevel, colors)}`;
    }
    let rightSide = model?.reasoning
      ? `${modelLabel} • ${thinkingLevel === "off" ? "thinking off" : thinkingLevel}`
      : modelLabel;

    const statsLeft = truncateToWidth(formatStatsLeft(context, theme), renderWidth, "...");
    const leftWidth = visibleWidth(statsLeft);
    if (model && footerData.getAvailableProviderCount() > 1) {
      const withProvider = `(${model.provider}) ${rightSide}`;
      if (visibleWidth(withProvider) <= renderWidth - leftWidth - 2) rightSide = withProvider;
    }
    let statsLine = statsLeft;
    const rightWidth = visibleWidth(rightSide);
    if (leftWidth + 2 + rightWidth <= renderWidth) {
      statsLine = statsLeft + " ".repeat(renderWidth - leftWidth - rightWidth) + rightSide;
    } else if (renderWidth - leftWidth - 2 > 0) {
      const right = truncateToWidth(rightSide, renderWidth - leftWidth - 2, "");
      statsLine =
        statsLeft + " ".repeat(Math.max(0, renderWidth - leftWidth - visibleWidth(right))) + right;
    }

    const directory = formatWorkingDirectory(context, footerData);
    const lines = [
      truncateToWidth(theme.fg("dim", directory), renderWidth, theme.fg("dim", "...")),
      theme.fg("dim", statsLeft) + dimPreservingFastLabel(theme, statsLine.slice(statsLeft.length)),
    ];

    const statuses = footerData.getExtensionStatuses();
    if (statuses.size > 0) {
      const text = [...statuses.entries()]
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([, status]) =>
          status
            .replace(/[\r\n\t]/g, " ")
            .replace(/ +/g, " ")
            .trim(),
        )
        .join(" ");
      lines.push(truncateToWidth(text, renderWidth, theme.fg("dim", "...")));
    }
    return lines;
  }
}
