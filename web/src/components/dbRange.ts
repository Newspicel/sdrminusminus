import type { DbWindow } from "./spectrumTraces";

export const DB_LIMIT: DbWindow = { min: -180, max: 20 };
export const DB_STEP = 1;
export const DB_MIN_SPAN = 5;

function clampDb(db: number, fallback: number): number {
  if (!Number.isFinite(db)) {
    return fallback;
  }
  return Math.min(DB_LIMIT.max, Math.max(DB_LIMIT.min, Math.round(db)));
}

export function withFloor(window: DbWindow, db: number): DbWindow {
  const min = Math.min(clampDb(db, DB_LIMIT.min), DB_LIMIT.max - DB_MIN_SPAN);
  return { min, max: Math.max(clampDb(window.max, DB_LIMIT.max), min + DB_MIN_SPAN) };
}

export function withCeiling(window: DbWindow, db: number): DbWindow {
  const max = Math.max(clampDb(db, DB_LIMIT.max), DB_LIMIT.min + DB_MIN_SPAN);
  return { min: Math.min(clampDb(window.min, DB_LIMIT.min), max - DB_MIN_SPAN), max };
}

export function clampWindow(window: DbWindow): DbWindow {
  return withCeiling(withFloor(window, window.min), window.max);
}
