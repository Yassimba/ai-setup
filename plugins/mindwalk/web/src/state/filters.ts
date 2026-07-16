import type { SessionMeta } from "../types";

export interface SessionFilters {
  hideEmpty: boolean;
  harness?: string;
}

const STORAGE_KEY = "mindwalk.sessionFilters";

// activeKey keeps an explicitly opened session visible even when it has no
// calls; the harness filter is deliberate user intent, so it still applies
export function sessionVisible(session: SessionMeta, filters: SessionFilters, activeKey?: string): boolean {
  if (filters.harness && session.harness !== filters.harness) return false;
  if (filters.hideEmpty && session.eventCount === 0 && session.key !== activeKey) return false;
  return true;
}

export function loadFilters(): SessionFilters {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) {
      const parsed = JSON.parse(raw) as Partial<SessionFilters>;
      return {
        hideEmpty: parsed.hideEmpty !== false,
        harness: typeof parsed.harness === "string" ? parsed.harness : undefined
      };
    }
  } catch {
    // corrupted storage: fall through to defaults
  }
  return { hideEmpty: true };
}

export function saveFilters(filters: SessionFilters): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(filters));
  } catch {
    // storage unavailable: filters reset on next load
  }
}
