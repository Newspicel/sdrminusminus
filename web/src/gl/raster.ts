// Pixel arithmetic for the shared scope renderer (CANVAS §7): how many device pixels a plot
// gets, and which row of the history ring the next spectrum frame lands in. Number in, number
// out — the sizing rules are testable, the GL calls wrapped around them are not.

/** Retina is worth the fill rate; past 2× the extra samples are below the resolution of the eye
 * and a Pi-class browser pays for them anyway (PLAN §1: the Pi is the floor). */
const MAX_DPR = 2;
/** Ceiling on dpr × canvas zoom: React Flow magnifies a node with a CSS transform, so a plot at
 * 2× zoom on a 2× display would otherwise ask for four device pixels per layout pixel — sixteen
 * times the buffer, for sharpness nobody can see. */
const MAX_PIXEL_RATIO = 3;
/** Floor, so a node zoomed far out still draws a recognisable plot rather than four rows. */
const MIN_PIXEL_RATIO = 0.5;
/** Ratios snap to eighths. Zoom is continuous, and a backing store resized on every frame of a
 * zoom gesture is reallocated and cleared on every frame of it. */
const RATIO_STEPS = 8;

/** How much larger an element is drawn than it is laid out — the scale of every CSS transform
 * above it. React Flow's zoom is the one that matters here; on the rack there is none. */
export function zoomOf(renderedPx: number, layoutPx: number): number {
  return renderedPx > 0 && layoutPx > 0 ? renderedPx / layoutPx : 1;
}

/** Device pixels per layout pixel for a plot: the display's own ratio, magnified by the canvas
 * zoom so a zoomed node re-renders instead of being stretched as a bitmap (CANVAS §7). */
export function pixelRatio(dpr: number, zoom: number): number {
  const raw = Math.min(dpr > 0 ? dpr : 1, MAX_DPR) * (zoom > 0 ? zoom : 1);
  const snapped = Math.round(raw * RATIO_STEPS) / RATIO_STEPS;
  return Math.min(MAX_PIXEL_RATIO, Math.max(MIN_PIXEL_RATIO, snapped));
}

/** Backing-store extent for a CSS extent at this ratio. */
export function backingPx(cssPx: number, ratio: number): number {
  return cssPx > 0 ? Math.round(cssPx * ratio) : 0;
}

/** History rows a plot of this backing height shows: one row per *layout* pixel. Counting
 * backing rows would halve the scroll speed on a 2× display, and would make zooming the canvas
 * change how far back the waterfall reaches. Clamped to the ring, so the bottom edge can never
 * wrap onto the newest row. */
export function rowsForHeight(heightPx: number, ratio: number, rings: number): number {
  return Math.max(2, Math.min(rings, Math.round(heightPx / (ratio > 0 ? ratio : 1))));
}

export function nextRingRow(row: number, rings: number): number {
  return rings > 0 ? (row + 1) % rings : 0;
}

/** One axis of the shared drawing buffer: grow to whatever the largest visible plot needs, and
 * shrink only once it needs less than half. A buffer that tracked the requirement exactly would
 * be reallocated on every frame of a resize. */
export function fitExtent(current: number, required: number): number {
  if (required > current) {
    return required;
  }
  return required * 2 <= current ? Math.max(1, required) : current;
}
