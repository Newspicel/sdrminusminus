import { create } from "zustand";
import type { ChannelLevel, ServerEvent } from "./types";

export const FLUSH_MS = 100;

export const LEVEL_FLOOR_DB = -140;

export type SetLevels = Readonly<Record<number, ChannelLevel>>;

export interface LevelState {
  byDeviceSet: Readonly<Record<number, SetLevels>>;
  observe: (event: ServerEvent) => void;
  clear: (deviceSet: number) => void;
  reset: () => void;
}

let pending: Record<number, SetLevels> | null = null;
let timer: ReturnType<typeof setTimeout> | null = null;

export const useLevelStore = create<LevelState>((set) => {
  const flush = () => {
    timer = null;
    const staged = pending;
    pending = null;
    if (staged === null) {
      return;
    }
    set((state) => ({ byDeviceSet: { ...state.byDeviceSet, ...staged } }));
  };

  return {
    byDeviceSet: {},
    observe: (event: ServerEvent) => {
      if (event.type !== "ChannelLevels") {
        return;
      }
      const byChannel: Record<number, ChannelLevel> = {};
      for (const level of event.data.levels) {
        byChannel[level.channel] = level;
      }
      pending = { ...pending, [event.data.device_set]: byChannel };
      if (timer === null) {
        timer = setTimeout(flush, FLUSH_MS);
      }
    },
    clear: (deviceSet: number) => {
      if (pending !== null) {
        delete pending[deviceSet];
      }
      set((state) => {
        if (!(deviceSet in state.byDeviceSet)) {
          return state;
        }
        const { [deviceSet]: _dropped, ...rest } = state.byDeviceSet;
        return { byDeviceSet: rest };
      });
    },
    reset: () => {
      pending = null;
      if (timer !== null) {
        clearTimeout(timer);
        timer = null;
      }
      set({ byDeviceSet: {} });
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

export function formatLevel(db: number | undefined): string {
  if (db === undefined || !Number.isFinite(db) || db <= LEVEL_FLOOR_DB) {
    return "—";
  }
  return `${db.toFixed(1)} dB`;
}
