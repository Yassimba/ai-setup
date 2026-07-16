import type { CityMap, Target, Touch, Trace, TraceEvent } from "../types";

export const touchRank: Record<Touch, number> = {
  hit: 1,
  read: 2,
  edit: 3
};

export interface FilePlayback {
  touchByFile: Map<number, Touch>;
  touchByPath: Map<string, Touch>;
  visitsByFile: Map<number, number>;
  historyByPath: Map<string, TraceEvent[]>;
  recentTargets: Target[];
}

const RECENT_WINDOW = 12;

// Incremental playback state. Stepping the playhead forward applies only the
// events it crossed; scrubbing backward replays from the start once. Snapshots
// share the engine's live collections — consumers must treat them as read-only
// and re-read whenever the snapshot identity changes (one fresh wrapper per
// snapshotAt call).
export class PlaybackEngine {
  private readonly events: TraceEvent[];
  private readonly idByPath = new Map<string, number>();
  private cursor = -1;
  private readonly touchByFile = new Map<number, Touch>();
  private readonly touchByPath = new Map<string, Touch>();
  private readonly visitsByFile = new Map<number, number>();
  private readonly historyByPath = new Map<string, TraceEvent[]>();
  private readonly recentTargets: Target[] = [];

  constructor(trace: Trace | undefined, city: CityMap | undefined) {
    this.events = trace?.events ?? [];
    for (const file of city?.files ?? []) {
      this.idByPath.set(file.path, file.id);
    }
  }

  snapshotAt(seq: number): FilePlayback {
    const lastSeq = Math.min(seq, this.events.length - 1);
    if (lastSeq < this.cursor) this.reset();
    for (let i = this.cursor + 1; i <= lastSeq; i++) {
      this.apply(this.events[i]);
    }
    this.cursor = Math.max(this.cursor, lastSeq);
    return {
      touchByFile: this.touchByFile,
      touchByPath: this.touchByPath,
      visitsByFile: this.visitsByFile,
      historyByPath: this.historyByPath,
      recentTargets: this.recentTargets
    };
  }

  private reset() {
    this.cursor = -1;
    this.touchByFile.clear();
    this.touchByPath.clear();
    this.visitsByFile.clear();
    this.historyByPath.clear();
    this.recentTargets.length = 0;
  }

  private apply(event: TraceEvent) {
    for (const target of event.targets) {
      const prev = this.touchByPath.get(target.path);
      if (!prev || touchRank[target.touch] > touchRank[prev]) {
        this.touchByPath.set(target.path, target.touch);
      }
      const fileId = target.fileId ?? this.idByPath.get(target.path);
      if (fileId !== undefined) {
        const prevFile = this.touchByFile.get(fileId);
        if (!prevFile || touchRank[target.touch] > touchRank[prevFile]) {
          this.touchByFile.set(fileId, target.touch);
        }
        this.visitsByFile.set(fileId, (this.visitsByFile.get(fileId) ?? 0) + 1);
      }
      const history = this.historyByPath.get(target.path);
      if (history) history.push(event);
      else this.historyByPath.set(target.path, [event]);
    }
    if (event.targets.length > 0) {
      // the trail follows the strongest signal: skip weak (heuristic) targets
      const primary = event.targets.find((target) => !target.weak) ?? event.targets[0];
      this.recentTargets.push({ ...primary, fileId: primary.fileId ?? this.idByPath.get(primary.path) });
      if (this.recentTargets.length > RECENT_WINDOW) this.recentTargets.shift();
    }
  }
}
