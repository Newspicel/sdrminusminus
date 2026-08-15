import type {
  NanoVnaCalibration,
  NanoVnaComplex,
  NanoVnaDevice,
  NanoVnaDeviceReport,
  NanoVnaPoint,
  NanoVnaSweep,
  NanoVnaSweepRequest,
  NanoVnaSweepState,
  ToolRequest,
  ToolResponse,
} from "../../lib/types";

export const REFERENCE_OHMS = 50;

export type CalibrationStep =
  | { step: "status" }
  | { step: "reset" }
  | { step: "open" }
  | { step: "short" }
  | { step: "load" }
  | { step: "thru" }
  | { step: "isolation" }
  | { step: "finish" }
  | { step: "enable" }
  | { step: "disable" }
  | { step: "save"; slot: number }
  | { step: "recall"; slot: number };

export function nanoVnaDevicesRequest(): ToolRequest {
  return { tool: "nanovna", request: { action: "list_devices" } };
}

export function nanoVnaDescribeRequest(port: string): ToolRequest {
  return { tool: "nanovna", request: { action: "describe", port } };
}

export function nanoVnaSweepRequest(request: NanoVnaSweepRequest): ToolRequest {
  return { tool: "nanovna", request: { action: "sweep", ...request } };
}

export function nanoVnaCalibrateRequest(
  port: string,
  step: CalibrationStep,
  range?: NanoVnaSweepState,
): ToolRequest {
  return {
    tool: "nanovna",
    request: { action: "calibrate", port, ...(range === undefined ? {} : { range }), ...step },
  };
}

export function nanoVnaDevices(response: ToolResponse | undefined): NanoVnaDevice[] {
  if (response?.tool !== "nanovna" || response.result.kind !== "devices") {
    return [];
  }
  return response.result.devices;
}

export function nanoVnaIgnoredPorts(response: ToolResponse | undefined): string[] {
  if (response?.tool !== "nanovna" || response.result.kind !== "devices") {
    return [];
  }
  return response.result.ignored_ports;
}

export function nanoVnaReport(response: ToolResponse | undefined): NanoVnaDeviceReport | null {
  if (response?.tool !== "nanovna" || response.result.kind !== "device") {
    return null;
  }
  return response.result;
}

export function nanoVnaSweep(response: ToolResponse | undefined): NanoVnaSweep | null {
  if (response?.tool !== "nanovna" || response.result.kind !== "sweep") {
    return null;
  }
  return response.result;
}

export function nanoVnaCalibration(response: ToolResponse | undefined): NanoVnaCalibration | null {
  if (response?.tool !== "nanovna" || response.result.kind !== "calibration") {
    return null;
  }
  return response.result;
}

export function magnitude(value: NanoVnaComplex): number {
  return Math.hypot(value.re, value.im);
}

export function gainDb(value: NanoVnaComplex): number {
  const absolute = magnitude(value);
  return absolute > 0 ? 20 * Math.log10(absolute) : Number.NEGATIVE_INFINITY;
}

export function returnLossDb(value: NanoVnaComplex): number {
  return -gainDb(value);
}

export function phaseDeg(value: NanoVnaComplex): number {
  return (Math.atan2(value.im, value.re) * 180) / Math.PI;
}

export function vswr(value: NanoVnaComplex): number {
  const gamma = magnitude(value);
  return gamma < 1 ? (1 + gamma) / (1 - gamma) : Number.POSITIVE_INFINITY;
}

export function mismatchLossDb(value: NanoVnaComplex): number {
  const gamma = magnitude(value);
  const transmitted = 1 - gamma * gamma;
  return transmitted > 0 ? -10 * Math.log10(transmitted) : Number.POSITIVE_INFINITY;
}

export function impedance(
  value: NanoVnaComplex,
  referenceOhms = REFERENCE_OHMS,
): NanoVnaComplex | null {
  const denominatorRe = 1 - value.re;
  const denominatorIm = -value.im;
  const denominator = denominatorRe * denominatorRe + denominatorIm * denominatorIm;
  if (denominator === 0) {
    return null;
  }
  const numeratorRe = 1 + value.re;
  const numeratorIm = value.im;
  return {
    re: (referenceOhms * (numeratorRe * denominatorRe + numeratorIm * denominatorIm)) / denominator,
    im: (referenceOhms * (numeratorIm * denominatorRe - numeratorRe * denominatorIm)) / denominator,
  };
}

export function admittance(value: NanoVnaComplex): NanoVnaComplex | null {
  const z = impedance(value);
  if (z === null) {
    return null;
  }
  const squared = z.re * z.re + z.im * z.im;
  if (squared === 0) {
    return null;
  }
  return { re: z.re / squared, im: -z.im / squared };
}

export function equivalentComponent(
  reactanceOhms: number,
  frequencyHz: number,
): { kind: "capacitance" | "inductance"; value: number } | null {
  if (!Number.isFinite(reactanceOhms) || frequencyHz <= 0 || reactanceOhms === 0) {
    return null;
  }
  const omega = 2 * Math.PI * frequencyHz;
  return reactanceOhms < 0
    ? { kind: "capacitance", value: -1 / (omega * reactanceOhms) }
    : { kind: "inductance", value: reactanceOhms / omega };
}

export function qFactor(z: NanoVnaComplex | null): number {
  if (z === null || z.re === 0) {
    return Number.POSITIVE_INFINITY;
  }
  return Math.abs(z.im / z.re);
}

export function unwrappedPhase(
  points: readonly NanoVnaPoint[],
  pick: (point: NanoVnaPoint) => NanoVnaComplex,
): number[] {
  const phases: number[] = [];
  let previous = 0;
  let offset = 0;
  points.forEach((point, index) => {
    const value = pick(point);
    const raw = Math.atan2(value.im, value.re);
    if (index > 0) {
      const delta = raw + offset - previous;
      if (delta > Math.PI) {
        offset -= 2 * Math.PI;
      } else if (delta < -Math.PI) {
        offset += 2 * Math.PI;
      }
    }
    previous = raw + offset;
    phases.push(previous);
  });
  return phases;
}

export function groupDelays(points: readonly NanoVnaPoint[]): number[] {
  const phases = unwrappedPhase(points, (point) => point.s21);
  return points.map((_, index) => {
    const low = Math.max(0, index - 1);
    const high = Math.min(points.length - 1, index + 1);
    const lowPoint = points[low];
    const highPoint = points[high];
    const lowPhase = phases[low];
    const highPhase = phases[high];
    if (
      lowPoint === undefined ||
      highPoint === undefined ||
      lowPhase === undefined ||
      highPhase === undefined ||
      high === low
    ) {
      return Number.NaN;
    }
    const deltaOmega = 2 * Math.PI * (highPoint.frequency_hz - lowPoint.frequency_hz);
    return deltaOmega === 0 ? Number.NaN : -(highPhase - lowPhase) / deltaOmega;
  });
}

export function lowestVswrIndex(points: readonly NanoVnaPoint[]): number {
  let best = 0;
  for (let index = 1; index < points.length; index += 1) {
    const candidate = points[index];
    const current = points[best];
    if (
      candidate !== undefined &&
      current !== undefined &&
      vswr(candidate.s11) < vswr(current.s11)
    ) {
      best = index;
    }
  }
  return best;
}

export function formatDb(value: number): string {
  return Number.isFinite(value) ? `${value.toFixed(2)} dB` : value > 0 ? "∞ dB" : "−∞ dB";
}

export function formatVswr(value: number): string {
  return Number.isFinite(value) ? `${value.toFixed(3)}:1` : "∞";
}

export function formatImpedance(value: NanoVnaComplex | null): string {
  if (value === null || !Number.isFinite(value.re) || !Number.isFinite(value.im)) {
    return "—";
  }
  return `${value.re.toFixed(1)} ${value.im < 0 ? "−" : "+"} j${Math.abs(value.im).toFixed(1)} Ω`;
}

const SMALLEST_PREFIX = { factor: 1e-15, prefix: "f" } as const;

const SI_PREFIXES = [
  { factor: 1, prefix: "" },
  { factor: 1e-3, prefix: "m" },
  { factor: 1e-6, prefix: "µ" },
  { factor: 1e-9, prefix: "n" },
  { factor: 1e-12, prefix: "p" },
  SMALLEST_PREFIX,
] as const;

export function formatSi(value: number, unit: string, digits = 3): string {
  if (!Number.isFinite(value)) {
    return "—";
  }
  if (value === 0) {
    return `0 ${unit}`;
  }
  const magnitudeOf = Math.abs(value);
  const scale = SI_PREFIXES.find((entry) => magnitudeOf >= entry.factor) ?? SMALLEST_PREFIX;
  return `${(value / scale.factor).toFixed(digits)} ${scale.prefix}${unit}`;
}

export function formatNumber(value: number, digits = 3): string {
  return Number.isFinite(value) ? value.toFixed(digits) : "—";
}
