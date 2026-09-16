import { describe, expect, it } from "vitest";
import {
  formatBaud,
  formatBitRate,
  formatBytes,
  formatHz,
  formatMhz,
  formatSampleRate,
  formatSignedHz,
  fractionDigits,
  si,
} from "./format";

describe("si", () => {
  it("picks the prefix from the magnitude and trims trailing zeros", () => {
    expect(si(0, "Hz")).toBe("0 Hz");
    expect(si(50.5, "Hz")).toBe("50.5 Hz");
    expect(si(999_999, "Hz")).toBe("999.999 kHz");
    expect(si(446_006_300, "Hz")).toBe("446.0063 MHz");
    expect(si(1_890_400_000, "Hz")).toBe("1.8904 GHz");
    expect(si(1_000_000, "Hz")).toBe("1 MHz");
    expect(si(1_090_000_000, "Hz")).toBe("1.09 GHz");
  });

  it("keeps the sign and the prefix on a negative value", () => {
    expect(si(-12_500, "Hz")).toBe("-12.5 kHz");
  });

  it("refuses to print a number for a value no radio can produce", () => {
    expect(si(Number.NaN, "Hz")).toBe("? Hz");
    expect(si(Number.POSITIVE_INFINITY, "S/s")).toBe("? S/s");
  });
});

describe("unit wrappers", () => {
  it("names each quantity in its SI unit", () => {
    expect(formatHz(145_500_000)).toBe("145.5 MHz");
    expect(formatSampleRate(2_048_000)).toBe("2.048 MS/s");
    expect(formatSampleRate(250_000)).toBe("250 kS/s");
    expect(formatBitRate(96_000)).toBe("96 kbit/s");
    expect(formatBaud(1_200)).toBe("1.2 kBd");
    expect(formatBytes(32_800_000)).toBe("32.8 MB");
    expect(formatBytes(512)).toBe("512 B");
  });

  it("signs an offset with a real minus, not a hyphen", () => {
    expect(formatSignedHz(-12_500)).toBe("−12.5 kHz");
    expect(formatSignedHz(0)).toBe("+0 Hz");
  });

  it("keeps a fixed width for sorted columns", () => {
    expect(formatMhz(145_500_000)).toBe("145.5000 MHz");
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
