import { describe, expect, it } from "vitest";
import {
  formatHz,
  formatKhz,
  formatMhz,
  formatSignedKhz,
  fractionDigits,
  parseFrequencyHz,
} from "./format";

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
    expect(fractionDigits(0.00001)).toBe(5);
  });

  it("ignores the sign and falls back for a step no driver declared", () => {
    expect(fractionDigits(-0.25)).toBe(2);
    expect(fractionDigits(undefined)).toBe(6);
    expect(fractionDigits(0)).toBe(6);
    expect(fractionDigits(Number.NaN)).toBe(6);
  });
});

describe("parseFrequencyHz", () => {
  it("reads a bare number as megahertz", () => {
    expect(parseFrequencyHz("433.92")).toBe(433_920_000);
    expect(parseFrequencyHz(" 145 ")).toBe(145_000_000);
  });

  it("accepts a decimal comma", () => {
    expect(parseFrequencyHz("433,92")).toBe(433_920_000);
  });

  it("honours an explicit unit, case and spacing free", () => {
    expect(parseFrequencyHz("433920 kHz")).toBe(433_920_000);
    expect(parseFrequencyHz("433920000hz")).toBe(433_920_000);
    expect(parseFrequencyHz("1.2 GHZ")).toBe(1_200_000_000);
  });

  it("round-trips the shortest decimal form of a hertz value", () => {
    expect(parseFrequencyHz(`${433_012_345 / 1e6}`)).toBe(433_012_345);
  });

  it("rejects anything that is not a positive frequency", () => {
    expect(parseFrequencyHz("")).toBeNull();
    expect(parseFrequencyHz("0")).toBeNull();
    expect(parseFrequencyHz("-145")).toBeNull();
    expect(parseFrequencyHz("1.2.3")).toBeNull();
    expect(parseFrequencyHz("145 mhz extra")).toBeNull();
    expect(parseFrequencyHz("145 furlongs")).toBeNull();
  });
});
