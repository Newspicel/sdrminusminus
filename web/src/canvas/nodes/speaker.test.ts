import { describe, expect, it } from "vitest";
import { pcmPeakDb, speakerHealthShown } from "./speaker";

const QUIET = { bufferedMs: 0, trimmedMs: 0, droppedMs: 0, underruns: 0, suspended: false };

describe("pcmPeakDb", () => {
  it("reads the loudest sample in dBFS", () => {
    expect(pcmPeakDb(Float32Array.of(0.25, -1, 0.5))).toBeCloseTo(0);
    expect(pcmPeakDb(Float32Array.of(0.1, -0.05))).toBeCloseTo(-20);
  });

  it("calls silence minus infinity", () => {
    expect(pcmPeakDb(Float32Array.of(0, 0))).toBe(Number.NEGATIVE_INFINITY);
    expect(pcmPeakDb(new Float32Array(0))).toBe(Number.NEGATIVE_INFINITY);
  });
});

describe("speakerHealthShown", () => {
  it("hides a healthy speaker footer", () => {
    expect(speakerHealthShown(QUIET)).toBe(false);
  });

  it("shows any fault or a suspended output", () => {
    expect(speakerHealthShown({ ...QUIET, underruns: 1 })).toBe(true);
    expect(speakerHealthShown({ ...QUIET, droppedMs: 20 })).toBe(true);
    expect(speakerHealthShown({ ...QUIET, suspended: true })).toBe(true);
  });
});
