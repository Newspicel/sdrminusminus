import { describe, expect, it } from "vitest";
import type { SpectrumFrame } from "./frame";
import { measureSignalDbfs, mergeSurveySample, signalSurveyCsv } from "./signalSurvey";

function frame(over: Partial<SpectrumFrame> = {}): SpectrumFrame {
  return {
    streamId: 1,
    seq: 1,
    timestamp: 1n,
    centerHz: 100_000_000,
    spanHz: 1_000_000,
    dbMin: -120,
    dbMax: -20,
    bins: Uint8Array.from([0, 51, 102, 153, 204]),
    ...over,
  };
}

describe("measureSignalDbfs", () => {
  it("takes the peak bin inside the requested bandwidth", () => {
    expect(measureSignalDbfs(frame(), 100_000_000, 200_000)).toBeCloseTo(-80, 6);
    expect(measureSignalDbfs(frame(), 100_300_000, 400_000)).toBeCloseTo(-40, 6);
  });

  it("rejects a target outside the current receiver span and malformed frames", () => {
    expect(measureSignalDbfs(frame(), 101_000_000, 12_500)).toBeNull();
    expect(measureSignalDbfs(frame({ bins: new Uint8Array() }), 100_000_000, 12_500)).toBeNull();
  });
});

describe("survey cells", () => {
  it("averages repeated locations in linear power rather than dB", () => {
    const first = mergeSurveySample([], {
      latitude: 52.52,
      longitude: 13.405,
      levelDbfs: -40,
      measuredAt: 1,
    });
    const merged = mergeSurveySample(first, {
      latitude: 52.520_001,
      longitude: 13.405_001,
      levelDbfs: -60,
      measuredAt: 2,
      accuracyM: 3,
    });

    expect(merged).toHaveLength(1);
    expect(merged[0]?.levelDbfs).toBeCloseTo(-42.967, 3);
    expect(merged[0]).toMatchObject({ measuredAt: 2, observations: 2, accuracyM: 3 });
  });

  it("exports measurements with their units and observation count", () => {
    const csv = signalSurveyCsv(
      [
        {
          latitude: 52.52,
          longitude: 13.405,
          levelDbfs: -67.125,
          measuredAt: Date.parse("2026-08-15T10:00:00Z"),
          observations: 3,
          accuracyM: 4.25,
        },
      ],
      145_500_000,
      12_500,
    );
    expect(csv).toContain("accuracy_m,level_dbfs,observations");
    expect(csv).toContain(
      "2026-08-15T10:00:00.000Z,145500000,12500,52.5200000,13.4050000,4.3,-67.13,3",
    );
  });
});
