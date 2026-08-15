const MAX_DPR = 2;
const MAX_PIXEL_RATIO = 3;
const MIN_PIXEL_RATIO = 0.5;
const RATIO_STEPS = 8;

export function zoomOf(renderedPx: number, layoutPx: number): number {
  return renderedPx > 0 && layoutPx > 0 ? renderedPx / layoutPx : 1;
}

export function pixelRatio(dpr: number, zoom: number): number {
  const raw = Math.min(dpr > 0 ? dpr : 1, MAX_DPR) * (zoom > 0 ? zoom : 1);
  const snapped = Math.round(raw * RATIO_STEPS) / RATIO_STEPS;
  return Math.min(MAX_PIXEL_RATIO, Math.max(MIN_PIXEL_RATIO, snapped));
}

export function backingPx(cssPx: number, ratio: number): number {
  return cssPx > 0 ? Math.round(cssPx * ratio) : 0;
}

export function rowsForHeight(heightPx: number, ratio: number, rings: number): number {
  return Math.max(2, Math.min(rings, Math.round(heightPx / (ratio > 0 ? ratio : 1))));
}

export function nextRingRow(row: number, rings: number): number {
  return rings > 0 ? (row + 1) % rings : 0;
}

export function seedPlacement(
  count: number,
  rings: number,
): { skip: number; rows: number; write: number } {
  const rows = Math.max(0, Math.min(count, rings));
  return { skip: Math.max(0, count - rows), rows, write: rings > 0 ? rows % rings : 0 };
}

export function fitExtent(current: number, required: number): number {
  if (required > current) {
    return required;
  }
  return required * 2 <= current ? Math.max(1, required) : current;
}
