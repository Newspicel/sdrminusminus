import { verticalWheel, type WheelDelta } from "../canvas/wheel";

export interface SpectrumView {
  start: number;
  end: number;
}

export const FULL_VIEW: SpectrumView = { start: 0, end: 1 };

const MIN_WIDTH = 1 / 512;
const WHEEL_ZOOM = 1.2;

export function viewWidth(view: SpectrumView): number {
  return view.end - view.start;
}

export function isFullView(view: SpectrumView): boolean {
  return view.start <= 0 && view.end >= 1;
}

export function zoomView(view: SpectrumView, at: number, factor: number): SpectrumView {
  const width = viewWidth(view);
  const anchor = view.start + clamp01(at) * width;
  const next = Math.min(1, Math.max(MIN_WIDTH, width / factor));
  return slide({ start: anchor - clamp01(at) * next, end: anchor - clamp01(at) * next + next });
}

export function panView(view: SpectrumView, byScreenFraction: number): SpectrumView {
  const width = viewWidth(view);
  const delta = byScreenFraction * width;
  return slide({ start: view.start + delta, end: view.end + delta });
}

export function wheelView(
  view: SpectrumView,
  wheel: WheelDelta,
  at: number,
  widthPx: number,
): SpectrumView {
  if (verticalWheel(wheel)) {
    return zoomView(view, at, wheel.deltaY < 0 ? WHEEL_ZOOM : 1 / WHEEL_ZOOM);
  }
  return panView(view, wheel.deltaX / Math.max(1, widthPx));
}

export function viewToSpan(view: SpectrumView, at: number): number {
  return view.start + at * viewWidth(view);
}

export function spanToView(view: SpectrumView, fraction: number): number {
  return (fraction - view.start) / viewWidth(view);
}

export function offsetToSpan(offsetHz: number, spanHz: number): number {
  return 0.5 + offsetHz / spanHz;
}

export function spanToOffset(fraction: number, spanHz: number): number {
  return (fraction - 0.5) * spanHz;
}

export interface Tick {
  hz: number;
  at: number;
}

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

export function niceStep(raw: number): number {
  if (!(raw > 0)) {
    return 1;
  }
  const magnitude = 10 ** Math.floor(Math.log10(raw));
  const normalized = raw / magnitude;
  const nice = normalized < 1.5 ? 1 : normalized < 3 ? 2 : normalized < 7 ? 5 : 10;
  return nice * magnitude;
}

export const MARKER_LABEL_GAP = 0.18;

const LABEL_CHAR_PX = 6;
const LABEL_CHROME_PX = 16;

export function labelWidth(text: string, plotWidthPx: number): number {
  return plotWidthPx > 0
    ? (text.length * LABEL_CHAR_PX + LABEL_CHROME_PX) / plotWidthPx
    : MARKER_LABEL_GAP;
}

export function clusterMarkers<T extends { at: number; width: number }>(
  markers: readonly T[],
): T[][] {
  const clusters: T[][] = [];
  for (const marker of markers.toSorted((a, b) => a.at - b.at)) {
    const open = clusters.at(-1);
    const anchor = open?.[0];
    if (
      open === undefined ||
      anchor === undefined ||
      marker.at - anchor.at >= (anchor.width + marker.width) / 2
    ) {
      clusters.push([marker]);
    } else {
      open.push(marker);
    }
  }
  return clusters;
}

function slide(view: SpectrumView): SpectrumView {
  const width = Math.min(1, viewWidth(view));
  const start = Math.min(1 - width, Math.max(0, view.start));
  return { start, end: start + width };
}

function clamp01(value: number): number {
  return Math.min(1, Math.max(0, value));
}
