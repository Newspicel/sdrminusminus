import { describe, expect, it } from "vitest";
import { formatHz, formatKhz, formatMhz, formatSignedKhz, fractionDigits } from "./format";

describe("frequency formats", () => {
  it("switches unit at 1 MHz and trims trailing zeros", () => {
    expect(formatHz(145_500_000)).toBe("145.5 MHz");
    expect(formatHz(1_000_000)).toBe("1 MHz");
    expect(formatHz(999_000)).toBe("999 kHz");
  });

  it("keeps a fixed width for sorted columns", () => {
    expect(formatMhz(145_500_000)).toBe("145.5000 MHz");
  });

  it("signs an offset with a real minus, not a hyphen", () => {
    expect(formatKhz(12_500)).toBe("12.5 kHz");
    expect(formatSignedKhz(-12_500)).toBe("−12.5 kHz");
    expect(formatSignedKhz(0)).toBe("+0 kHz");
  });
});

describe("fractionDigits", () => {
  it("takes its precision from the step", () => {
    expect(fractionDigits(1)).toBe(0);
    expect(fractionDigits(50)).toBe(0);
    expect(fractionDigits(0.5)).toBe(1);
    expect(fractionDigits(0.05)).toBe(2);
    // The ADS-B reference position: three digits would be kilometres of error.
    expect(fractionDigits(0.00001)).toBe(5);
  });

  it("ignores the sign and falls back for a step no driver declared", () => {
    expect(fractionDigits(-0.25)).toBe(2);
    expect(fractionDigits(undefined)).toBe(6);
    expect(fractionDigits(0)).toBe(6);
    expect(fractionDigits(Number.NaN)).toBe(6);
  });
});
