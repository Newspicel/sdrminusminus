export function formatHz(hz: number): string {
  return hz >= 1e6
    ? `${trimZeros((hz / 1e6).toFixed(3))} MHz`
    : `${trimZeros((hz / 1e3).toFixed(1))} kHz`;
}

export function formatKhz(hz: number): string {
  return `${trimZeros((hz / 1e3).toFixed(3))} kHz`;
}

export function formatSignedKhz(hz: number): string {
  return `${hz < 0 ? "−" : "+"}${formatKhz(Math.abs(hz))}`;
}

export function formatMhz(hz: number): string {
  return `${(hz / 1e6).toFixed(4)} MHz`;
}

const UNIT_SCALE_HZ: Record<string, number> = { hz: 1, khz: 1e3, mhz: 1e6, ghz: 1e9 };

export function parseFrequencyHz(text: string): number | null {
  const match = /^(\d+(?:[.,]\d+)?)\s*(hz|khz|mhz|ghz)?$/i.exec(text.trim());
  if (match === null) {
    return null;
  }
  const [, digits = "", unit = "mhz"] = match;
  const scale = UNIT_SCALE_HZ[unit.toLowerCase()];
  const value = Number(digits.replace(",", "."));
  if (scale === undefined || !Number.isFinite(value) || value <= 0) {
    return null;
  }
  return value * scale;
}

function trimZeros(fixed: string): string {
  return fixed.replace(/\.?0+$/, "");
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
