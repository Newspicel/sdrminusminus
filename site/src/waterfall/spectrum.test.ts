import { describe, expect, it } from "vitest";
import { heat, keyed, type Signal, spectrumRow } from "./spectrum";

const carrier: Signal = {
  center: 0.5,
  width: 0.01,
  level: 0.8,
  period: 1,
  duty: 1,
  phase: 0,
  drift: 0,
};

const silent = () => 0;

describe("spectrumRow", () => {
  it("peaks at a keyed carrier", () => {
    const row = new Float32Array(200);
    spectrumRow(row, 0, [carrier], silent);
    expect(row[100]).toBeCloseTo(0.06 + 0.8 * 0.8, 5);
    expect(row[10]).toBeCloseTo(0.06, 5);
  });

  it("stays within range", () => {
    const row = new Float32Array(64);
    spectrumRow(row, 3, [{ ...carrier, level: 5 }], () => 0.99);
    expect(Math.max(...row)).toBeLessThanOrEqual(1);
    expect(Math.min(...row)).toBeGreaterThanOrEqual(0);
  });

  it("leaves an unkeyed burst out", () => {
    const burst = { ...carrier, period: 10, duty: 0.5 };
    const row = new Float32Array(200);
    spectrumRow(row, 7, [burst], silent);
    expect(row[100]).toBeCloseTo(0.06, 5);
  });
});

describe("keyed", () => {
  it("follows duty and phase", () => {
    const burst = { ...carrier, period: 10, duty: 0.3, phase: 5 };
    expect(keyed(burst, 0)).toBe(false);
    expect(keyed(burst, 5)).toBe(true);
    expect(keyed(burst, 8)).toBe(false);
  });
});

describe("heat", () => {
  it("maps floor to transparent and peak to opaque", () => {
    const pixels = new Uint8ClampedArray(8);
    heat(0, pixels, 0);
    heat(1, pixels, 4);
    expect(pixels[3]).toBe(0);
    expect(pixels[7]).toBe(255);
  });
});
