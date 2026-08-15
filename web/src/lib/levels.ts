// Live channel signal levels. `ChannelLevels` bypasses TanStack Query for the same reason
// `ScannerUpdate` does: levels move continuously, and one full-state refetch per reading would
// cost far more than the reading. Nothing here is authoritative — a level is a measurement, and a
// client that misses one simply draws the next.
import { create } from "zustand";
import type { ChannelLevel, ServerEvent } from "./types";

/** Updates are staged and published at most this often. The server pushes ten a second, which is
 * what a meter needs to *be* a meter; re-rendering React that often is not. */
export const FLUSH_MS = 100;

/** dBFS a meter shows nothing below — the level a silent channel reports. Mirrors the engine's
 * `LEVEL_FLOOR_DB`, so a floored reading is recognisable as "nothing here" rather than drawn as a
 * very quiet signal. */
export const LEVEL_FLOOR_DB = -140;

/** Levels of one device set, keyed by channel id. */
export type SetLevels = Readonly<Record<number, ChannelLevel>>;

export interface LevelState {
  /** Latest reading per device set. Absent means "nothing measured yet", which is what a set
   * with no channels reports too. */
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
      // Replaced, never merged: the message carries every channel the set has, so a channel
      // missing from it is one that is gone.
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

/**
 * Where a level sits on a meter's unit width.
 *
 * The scale is deliberately not linear in dB across the whole range: almost everything a receiver
 * ever shows lives in the top 60 dB, and a meter that spent half its width on levels no signal
 * reaches would waste the half that matters.
 */
export function levelUnit(db: number, floorDb = -90): number {
  if (!Number.isFinite(db) || db <= floorDb) {
    return 0;
  }
  return Math.min(1, (db - floorDb) / -floorDb);
}

/**
 * Where the gate this level is measured against actually sits.
 *
 * The measurement wins over the setting wherever there is one: a channel tracking its own noise
 * floor has a threshold that moves, and `squelch_db` on the settings is then only what it falls
 * back to when tracking is switched off. The setting still stands in before the first reading
 * arrives, so the meter's notch does not appear a tenth of a second after the channel does.
 */
export function gateDb(
  level: ChannelLevel | undefined,
  settingDb: number | null | undefined,
): number | null {
  return level?.squelch_db ?? settingDb ?? null;
}

/** Whether the channel is above its gate — what the meter fills in the open colour for. */
export function gateOpen(
  level: ChannelLevel | undefined,
  settingDb: number | null | undefined,
): boolean {
  const gate = gateDb(level, settingDb);
  return level !== undefined && gate !== null && level.level_db >= gate;
}

/** `−42.1 dB`, or a dash for a channel that has measured nothing. */
export function formatLevel(db: number | undefined): string {
  if (db === undefined || !Number.isFinite(db) || db <= LEVEL_FLOOR_DB) {
    return "—";
  }
  return `${db.toFixed(1)} dB`;
}
