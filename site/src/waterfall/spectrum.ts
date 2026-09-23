export interface Signal {
  center: number;
  width: number;
  level: number;
  period: number;
  duty: number;
  phase: number;
  drift: number;
}

export type Random = () => number;

export const SIGNALS: Signal[] = [
  { center: 0.12, width: 0.03, level: 0.75, period: 1, duty: 1, phase: 0, drift: 0.002 },
  { center: 0.21, width: 0.028, level: 0.55, period: 1, duty: 1, phase: 0, drift: 0.001 },
  { center: 0.33, width: 0.003, level: 0.7, period: 1, duty: 1, phase: 0, drift: 0.004 },
  { center: 0.44, width: 0.008, level: 0.8, period: 70, duty: 0.25, phase: 12, drift: 0 },
  { center: 0.5, width: 0.008, level: 0.65, period: 110, duty: 0.15, phase: 40, drift: 0 },
  { center: 0.61, width: 0.004, level: 0.6, period: 36, duty: 0.8, phase: 0, drift: 0 },
  { center: 0.64, width: 0.004, level: 0.5, period: 36, duty: 0.8, phase: 18, drift: 0 },
  { center: 0.67, width: 0.004, level: 0.55, period: 36, duty: 0.8, phase: 9, drift: 0 },
  { center: 0.79, width: 0.022, level: 0.6, period: 160, duty: 0.6, phase: 30, drift: 0 },
  { center: 0.9, width: 0.002, level: 0.75, period: 1, duty: 1, phase: 0, drift: 0.008 },
];

type Rgba = readonly [number, number, number, number];

const STOPS: readonly Rgba[] = [
  [0, 0, 0, 0],
  [40, 70, 170, 90],
  [60, 170, 225, 190],
  [245, 215, 120, 255],
];

export function keyed(signal: Signal, frame: number): boolean {
  return (frame + signal.phase) % signal.period < signal.period * signal.duty;
}

export function spectrumRow(
  out: Float32Array,
  frame: number,
  signals: Signal[],
  random: Random,
): void {
  const bins = out.length;
  for (let bin = 0; bin < bins; bin++) {
    out[bin] = 0.06 + random() * 0.14;
  }
  for (const signal of signals) {
    if (keyed(signal, frame)) {
      addSignal(out, signal, frame, random);
    }
  }
}

function addSignal(out: Float32Array, signal: Signal, frame: number, random: Random): void {
  const bins = out.length;
  const center = (signal.center + signal.drift * Math.sin(frame / 90)) * bins;
  const spread = Math.max(0.6, signal.width * bins);
  const from = Math.max(0, Math.floor(center - spread * 3));
  const to = Math.min(bins - 1, Math.ceil(center + spread * 3));
  for (let bin = from; bin <= to; bin++) {
    const distance = (bin - center) / spread;
    const shape = Math.exp((-distance * distance) / 2);
    out[bin] = Math.min(1, (out[bin] ?? 0) + signal.level * shape * (0.8 + random() * 0.2));
  }
}

export function heat(value: number, pixels: Uint8ClampedArray, offset: number): void {
  const scaled = Math.min(1, Math.max(0, value)) * (STOPS.length - 1);
  const index = Math.min(STOPS.length - 2, Math.floor(scaled));
  const mix = scaled - index;
  const low = STOPS[index];
  const high = STOPS[index + 1];
  if (low === undefined || high === undefined) {
    return;
  }
  pixels[offset] = low[0] + (high[0] - low[0]) * mix;
  pixels[offset + 1] = low[1] + (high[1] - low[1]) * mix;
  pixels[offset + 2] = low[2] + (high[2] - low[2]) * mix;
  pixels[offset + 3] = low[3] + (high[3] - low[3]) * mix;
}
