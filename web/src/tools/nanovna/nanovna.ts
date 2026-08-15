import type {
  NanoVnaComplex,
  NanoVnaDevice,
  NanoVnaPoint,
  NanoVnaSweep,
  NanoVnaSweepRequest,
  ToolRequest,
  ToolResponse,
} from "../../lib/types";

export function nanoVnaDevicesRequest(): ToolRequest {
  return { tool: "nanovna", request: { action: "list_devices" } };
}

export function nanoVnaSweepRequest(request: NanoVnaSweepRequest): ToolRequest {
  return { tool: "nanovna", request: { action: "sweep", ...request } };
}

export function nanoVnaDevices(response: ToolResponse | undefined): NanoVnaDevice[] {
  if (response?.tool !== "nanovna" || response.result.kind !== "devices") {
    return [];
  }
  return response.result.devices;
}

export function nanoVnaSweep(response: ToolResponse | undefined): NanoVnaSweep | null {
  if (response?.tool !== "nanovna" || response.result.kind !== "sweep") {
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

export function impedance(value: NanoVnaComplex, referenceOhms = 50): NanoVnaComplex | null {
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
  return Number.isFinite(value) ? `${value.toFixed(2)} dB` : "—";
}

export function formatVswr(value: number): string {
  return Number.isFinite(value) ? `${value.toFixed(2)}:1` : "∞";
}

export function formatImpedance(value: NanoVnaComplex | null): string {
  if (value === null || !Number.isFinite(value.re) || !Number.isFinite(value.im)) {
    return "—";
  }
  return `${value.re.toFixed(1)} ${value.im < 0 ? "−" : "+"} j${Math.abs(value.im).toFixed(1)} Ω`;
}
