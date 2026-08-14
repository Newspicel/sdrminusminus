
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

// Fixed width (no trim) so a sorted bookmark column stays digit-aligned.
export function formatMhz(hz: number): string {
  return `${(hz / 1e6).toFixed(4)} MHz`;
}

// Safe on `toFixed` output only: it always carries a decimal point, so the trim can't eat
// integer zeros.
function trimZeros(fixed: string): string {
  return fixed.replace(/\.?0+$/, "");
}

/** How many decimals a numeric field may show, read off the step it advances by.
 *
 * `Intl.NumberFormat` defaults to three, which would silently round a five-decimal reference
 * position down to metres of error; deriving from the step instead also hides the trailing
 * float dust that stepping by 0.05 accumulates. Steps below this file's `toExponential` range
 * are not reachable from any setting the server declares. */
export function fractionDigits(step: number | undefined): number {
  if (step === undefined || !Number.isFinite(step) || step === 0) {
    return DEFAULT_FRACTION_DIGITS;
  }
  const [mantissa = "", exponent = "0"] = Math.abs(step).toExponential().split("e");
  const decimals = (mantissa.split(".")[1] ?? "").length;
  return Math.min(20, Math.max(0, decimals - Number(exponent)));
}

/** A setting whose driver declares no step: fine enough for any real device value, coarse
 * enough to keep float dust out of the field. */
const DEFAULT_FRACTION_DIGITS = 6;
