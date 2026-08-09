// Shared frequency display formats (PLAN §10: mono tabular numerals for data columns).

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
