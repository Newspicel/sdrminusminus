import { create } from "zustand";
import type { ChannelLevel, ServerEvent } from "./types";

export const FLUSH_MS = 100;

export const LEVEL_FLOOR_DB = -140;

export type SetLevels = Readonly<Record<number, ChannelLevel>>;

export type LanePeaks = Readonly<Record<number, number>>;

export interface LevelState {
  byDeviceSet: Readonly<Record<number, SetLevels>>;
  lanesByDeviceSet: Readonly<Record<number, LanePeaks>>;
  observe: (event: ServerEvent) => void;
  clear: (deviceSet: number) => void;
  reset: () => void;
}

let pending: Record<number, SetLevels> | null = null;
let pendingLanes: Record<number, LanePeaks> | null = null;
let timer: ReturnType<typeof setTimeout> | null = null;

function without<T>(record: Readonly<Record<number, T>>, key: number): Readonly<Record<number, T>> {
  const { [key]: _dropped, ...rest } = record;
  return rest;
}

export const useLevelStore = create<LevelState>((set) => {
  const flush = () => {
    timer = null;
    const staged = pending;
    const stagedLanes = pendingLanes;
    pending = null;
    pendingLanes = null;
    if (staged === null && stagedLanes === null) {
      return;
    }
    set((state) => ({
      byDeviceSet: { ...state.byDeviceSet, ...staged },
      lanesByDeviceSet: { ...state.lanesByDeviceSet, ...stagedLanes },
    }));
  };

  return {
    byDeviceSet: {},
    lanesByDeviceSet: {},
    observe: (event: ServerEvent) => {
      if (event.type !== "ChannelLevels") {
        return;
      }
      const byChannel: Record<number, ChannelLevel> = {};
      for (const level of event.data.levels) {
        byChannel[level.channel] = level;
      }
      const byLane: Record<number, number> = {};
      for (const lane of event.data.lanes ?? []) {
        byLane[lane.stream] = lane.peak_db;
      }
      pending = { ...pending, [event.data.device_set]: byChannel };
      pendingLanes = { ...pendingLanes, [event.data.device_set]: byLane };
      if (timer === null) {
        timer = setTimeout(flush, FLUSH_MS);
      }
    },
    clear: (deviceSet: number) => {
      if (pending !== null) {
        delete pending[deviceSet];
      }
      if (pendingLanes !== null) {
        delete pendingLanes[deviceSet];
      }
      set((state) => {
        if (!(deviceSet in state.byDeviceSet) && !(deviceSet in state.lanesByDeviceSet)) {
          return state;
        }
        return {
          byDeviceSet: without(state.byDeviceSet, deviceSet),
          lanesByDeviceSet: without(state.lanesByDeviceSet, deviceSet),
        };
      });
    },
    reset: () => {
      pending = null;
      pendingLanes = null;
      if (timer !== null) {
        clearTimeout(timer);
        timer = null;
      }
      set({ byDeviceSet: {}, lanesByDeviceSet: {} });
    },
  };
});

export function levelUnit(db: number, floorDb = -90): number {
  if (!Number.isFinite(db) || db <= floorDb) {
    return 0;
  }
  return Math.min(1, (db - floorDb) / -floorDb);
}

export function gateDb(
  level: ChannelLevel | undefined,
  settingDb: number | null | undefined,
): number | null {
  return level?.squelch_db ?? settingDb ?? null;
}

export function gateOpen(
  level: ChannelLevel | undefined,
  settingDb: number | null | undefined,
): boolean {
  const gate = gateDb(level, settingDb);
  return level !== undefined && gate !== null && level.level_db >= gate;
}

export function heardHz(frequencyHz: number, level: ChannelLevel | undefined): number {
  return frequencyHz + Math.round(level?.shift_hz ?? 0);
}

export function formatLevel(db: number | undefined): string {
  if (db === undefined || !Number.isFinite(db) || db <= LEVEL_FLOOR_DB) {
    return "-";
  }
  return `${db.toFixed(1)} dB`;
}
