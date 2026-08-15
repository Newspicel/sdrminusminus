export interface SpectrumView {
  start: number;
  end: number;
}

export const FULL_VIEW: SpectrumView = { start: 0, end: 1 };

/** 512× is where a 1024-bin frame has two bins per screen and magnifying further shows the
 * interpolation rather than the signal. */
const MIN_WIDTH = 1 / 512;

export function viewWidth(view: SpectrumView): number {
  return view.end - view.start;
}

export function isFullView(view: SpectrumView): boolean {
  return view.start <= 0 && view.end >= 1;
}

/** Zoom about a screen position, so the frequency under the cursor stays under the cursor —
 * the fixed point is what makes wheel-zoom feel like moving a lens rather than a scrollbar. */
export function zoomView(view: SpectrumView, at: number, factor: number): SpectrumView {
  const width = viewWidth(view);
  const anchor = view.start + clamp01(at) * width;
  const next = Math.min(1, Math.max(MIN_WIDTH, width / factor));
  return slide({ start: anchor - clamp01(at) * next, end: anchor - clamp01(at) * next + next });
}

/** Pan by a fraction of the *screen* width, so a drag moves the spectrum exactly as far as the
 * pointer moved however far it is zoomed in. */
export function panView(view: SpectrumView, byScreenFraction: number): SpectrumView {
  const width = viewWidth(view);
  const delta = byScreenFraction * width;
  return slide({ start: view.start + delta, end: view.end + delta });
}

/** Screen fraction → device-span fraction. */
export function viewToSpan(view: SpectrumView, at: number): number {
  return view.start + at * viewWidth(view);
}

/** Device-span fraction → screen fraction. Values outside [0, 1] are off-screen, which the
 * caller needs to know rather than have clamped away. */
export function spanToView(view: SpectrumView, fraction: number): number {
  return (fraction - view.start) / viewWidth(view);
}

/** Device-span fraction of a frequency offset from centre. */
export function offsetToSpan(offsetHz: number, spanHz: number): number {
  return 0.5 + offsetHz / spanHz;
}

export function spanToOffset(fraction: number, spanHz: number): number {
  return (fraction - 0.5) * spanHz;
}

export interface Tick {
  hz: number;
  /** Screen fraction, already inside [0, 1]. */
  at: number;
}

/** Axis ticks at "nice" round frequencies inside the visible window, spaced so labels do not
 * collide: `target` is how many the axis has room for. */
export function frequencyTicks(
  centerHz: number,
  spanHz: number,
  view: SpectrumView,
  target: number,
): Tick[] {
  const visible = spanHz * viewWidth(view);
  if (!(visible > 0) || !(target >= 1)) {
    return [];
  }
  const lowHz = centerHz + spanToOffset(view.start, spanHz);
  const step = niceStep(visible / target);
  const ticks: Tick[] = [];
  for (let hz = Math.ceil(lowHz / step) * step; hz <= lowHz + visible; hz += step) {
    ticks.push({ hz, at: (hz - lowHz) / visible });
  }
  return ticks;
}

/** dB gridlines over the frame's own range, on the same nice-number ladder. */
export function decibelTicks(dbMin: number, dbMax: number, target: number): number[] {
  if (!(dbMax > dbMin) || !(target >= 1)) {
    return [];
  }
  const step = niceStep((dbMax - dbMin) / target);
  const ticks: number[] = [];
  for (let db = Math.ceil(dbMin / step) * step; db <= dbMax; db += step) {
    ticks.push(db);
  }
  return ticks;
}

/** 1, 2, 5 × 10ⁿ — the ladder whose labels a reader can interpolate between without arithmetic. */
export function niceStep(raw: number): number {
  if (!(raw > 0)) {
    return 1;
  }
  const magnitude = 10 ** Math.floor(Math.log10(raw));
  const normalized = raw / magnitude;
  const nice = normalized < 1.5 ? 1 : normalized < 3 ? 2 : normalized < 7 ? 5 : 10;
  return nice * magnitude;
}

/** How far apart, as a fraction of the plot's width, two marker labels must sit to be legible
 * side by side. Roughly the widest `MODE +000.0k` label on a scope at its default size. */
export const MARKER_LABEL_GAP = 0.18;

/**
 * Markers grouped into the sets whose labels would land on top of each other, each ascending by
 * position and the groups themselves ascending by their first member.
 *
 * Several channels on one frequency is ordinary — a DMR carrier and its trunk follower, an NFM
 * and the tone decoder watching it — and their labels coincide exactly. A group is drawn as one
 * label that opens into the whole set, rather than as a permanent stack: six labels down the
 * middle of the trace hide more of the signal than the collision did.
 *
 * Membership is measured against the group's *first* marker, not the previous one, so a group
 * never spans more than `gap` — the one label stands where all of them are.
 */
export function clusterMarkers<T extends { at: number }>(
  markers: readonly T[],
  gap = MARKER_LABEL_GAP,
): T[][] {
  const clusters: T[][] = [];
  for (const marker of markers.toSorted((a, b) => a.at - b.at)) {
    const open = clusters.at(-1);
    const anchor = open?.[0];
    if (open === undefined || anchor === undefined || marker.at - anchor.at >= gap) {
      clusters.push([marker]);
    } else {
      open.push(marker);
    }
  }
  return clusters;
}

/** Keep a view inside the span without changing its width: a pan that runs off the end stops
 * there instead of shrinking, which would silently change the zoom level mid-drag. */
function slide(view: SpectrumView): SpectrumView {
  const width = Math.min(1, viewWidth(view));
  const start = Math.min(1 - width, Math.max(0, view.start));
  return { start, end: start + width };
}

function clamp01(value: number): number {
  return Math.min(1, Math.max(0, value));
}
