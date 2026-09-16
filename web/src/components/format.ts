const PREFIXES: ReadonlyArray<readonly [number, string]> = [
  [1e9, "G"],
  [1e6, "M"],
  [1e3, "k"],
  [1, ""],
];

const DECIMALS = 9;

export function si(value: number, unit: string): string {
  if (!Number.isFinite(value)) {
    return `? ${unit}`;
  }
  const magnitude = Math.abs(value);
  const [scale, prefix] = PREFIXES.find(([step]) => magnitude >= step) ?? [1, ""];
  return `${trimZeros((value / scale).toFixed(DECIMALS))} ${prefix}${unit}`;
}

export function formatHz(hz: number): string {
  return si(hz, "Hz");
}

export function formatSignedHz(hz: number): string {
  return `${hz < 0 ? "−" : "+"}${formatHz(Math.abs(hz))}`;
}

export function formatSampleRate(samplesPerSecond: number): string {
  return si(samplesPerSecond, "S/s");
}

export function formatBitRate(bitsPerSecond: number): string {
  return si(bitsPerSecond, "bit/s");
}

export function formatBaud(symbolsPerSecond: number): string {
  return si(symbolsPerSecond, "Bd");
}

export function formatBytes(bytes: number): string {
  return si(bytes, "B");
}

export function formatMhz(hz: number): string {
  return `${(hz / 1e6).toFixed(4)} MHz`;
}

function trimZeros(fixed: string): string {
  return fixed.includes(".") ? fixed.replace(/\.?0+$/, "") : fixed;
}

export function fractionDigits(step: number | undefined): number {
  if (step === undefined || !Number.isFinite(step) || step === 0) {
    return DEFAULT_FRACTION_DIGITS;
  }
  const [mantissa = "", exponent = "0"] = Math.abs(step).toExponential().split("e");
  const decimals = (mantissa.split(".")[1] ?? "").length;
  return Math.min(20, Math.max(0, decimals - Number(exponent)));
}

const DEFAULT_FRACTION_DIGITS = 6;
