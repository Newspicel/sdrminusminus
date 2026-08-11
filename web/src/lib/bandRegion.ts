// Which band plan the ruler and the explorer read (FEATURES §5). Held per browser, next to the
// theme and the colormap, and for the same reason the theme is: a region is a property of where
// the antenna is, not of the patch drawn on top of it. Two operators on one server in two
// countries — a remote receiver is the ordinary case — must not fight over one stored setting.
//
// The server holds no region state at all: it serves every region's table and takes the id in
// the path. If a station location ever arrives from a GPS backend, it subsumes this without the
// wire changing shape.
import { useSyncExternalStore } from "react";

const KEY = "sdrmm.bandRegion";
/** Whether the ruler is drawn at all. Separate from the region: an operator who knows the band
 * plan wants the pixels back, without forgetting where they are. */
const RULER_KEY = "sdrmm.bandRuler";

export interface BandRegionState {
  /** `null` until the operator or the server's default has chosen one. */
  region: string | null;
  ruler: boolean;
}

const listeners = new Set<() => void>();
let state: BandRegionState = { region: read(KEY), ruler: read(RULER_KEY) !== "off" };

export function useBandRegion(): BandRegionState {
  return useSyncExternalStore(subscribe, snapshot, snapshot);
}

export function setBandRegion(region: string): void {
  write(KEY, region);
  state = { ...state, region };
  emit();
}

export function setBandRuler(ruler: boolean): void {
  write(RULER_KEY, ruler ? "on" : "off");
  state = { ...state, ruler };
  emit();
}

/** Adopt the server's default, but never over a stored choice — this runs on every mount of
 * anything that reads the plan, and it must not undo what the operator picked. */
export function defaultBandRegion(region: string): void {
  if (state.region === null) {
    state = { ...state, region };
    emit();
  }
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function snapshot(): BandRegionState {
  return state;
}

function emit(): void {
  for (const listener of listeners) {
    listener();
  }
}

function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // A blocked store costs the preference on the next load, not this session.
  }
}
