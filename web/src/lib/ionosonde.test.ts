import { describe, expect, it } from "vitest";
import { compareCells, forecastAgreement, forecastAt } from "./ionosonde";
import type { PropagationCell } from "./propagation";
import type { IonosondeStation } from "./types";

function station(over: Partial<IonosondeStation> = {}): IonosondeStation {
  return {
    code: "AA000",
    name: "Somewhere",
    latitude: 50,
    longitude: 0,
    muf3000_mhz: 20,
    measured_at: "2026-08-16T12:00:00Z",
    ...over,
  };
}

function cell(over: Partial<PropagationCell> = {}): PropagationCell {
  return {
    key: "IO91",
    latitude: 51.5,
    longitude: -1,
    weight: 1,
    decodes: 1,
    callsigns: 1,
    bestFreqHz: 14_074_000,
    bestSnrDb: -12,
    measuredMuf3000Mhz: 18,
    medianDistanceKm: 3_000,
    lastSeen: 0,
    ...over,
  };
}

describe("interpolating the ionosonde network", () => {
  it("averages two equally close sites", () => {
    const forecast = forecastAt(
      [
        station({ code: "W", longitude: -5, muf3000_mhz: 18 }),
        station({ code: "E", longitude: 5, muf3000_mhz: 22 }),
      ],
      50,
      0,
    );
    expect(forecast?.muf3000Mhz).toBeCloseTo(20, 6);
    expect(forecast?.stations).toBe(2);
  });

  it("leans towards the nearer site", () => {
    const forecast = forecastAt(
      [
        station({ code: "NEAR", longitude: -1, muf3000_mhz: 30 }),
        station({ code: "FAR", longitude: 10, muf3000_mhz: 10 }),
      ],
      50,
      0,
    );
    expect(forecast?.muf3000Mhz).toBeGreaterThan(25);
    expect(forecast?.nearest).toBe("Somewhere");
    expect(forecast?.nearestKm).toBeLessThan(100);
  });

  it("says nothing rather than guessing from one distant site", () => {
    expect(forecastAt([station()], 50, 0)).toBeNull();
    expect(forecastAt([], 50, 0)).toBeNull();
    expect(
      forecastAt(
        [station({ latitude: -50, longitude: 170 }), station({ latitude: -40, longitude: 160 })],
        50,
        0,
      ),
    ).toBeNull();
  });

  it("trusts a confident site over a doubtful one at the same range", () => {
    const forecast = forecastAt(
      [
        station({ code: "SURE", longitude: -5, muf3000_mhz: 30, confidence: 100 }),
        station({ code: "SHAKY", longitude: 5, muf3000_mhz: 10, confidence: 10 }),
      ],
      50,
      0,
    );
    expect(forecast?.muf3000Mhz).toBeGreaterThan(20);
  });
});

describe("comparing what was heard against what was forecast", () => {
  const sondes = [
    station({ code: "W", latitude: 51, longitude: -3, muf3000_mhz: 16 }),
    station({ code: "E", latitude: 52, longitude: 1, muf3000_mhz: 16 }),
  ];

  it("reports the gap between a measured floor and the forecast", () => {
    const compared = compareCells([cell({ measuredMuf3000Mhz: 18 })], sondes);
    expect(compared).toHaveLength(1);
    expect(compared[0]?.forecast.muf3000Mhz).toBeCloseTo(16, 6);
    expect(compared[0]?.deltaMhz).toBeCloseTo(2, 6);
  });

  it("skips a cell with no measurement and one with no nearby site", () => {
    expect(compareCells([cell({ measuredMuf3000Mhz: null })], sondes)).toHaveLength(0);
    expect(compareCells([cell({ latitude: -40, longitude: 150 })], sondes)).toHaveLength(0);
  });

  it("summarises how often the receiver beat the network", () => {
    const compared = compareCells(
      [
        cell({ key: "IO91", measuredMuf3000Mhz: 18 }),
        cell({ key: "IO92", latitude: 52.5, measuredMuf3000Mhz: 20 }),
        cell({ key: "IO90", latitude: 50.5, measuredMuf3000Mhz: 12 }),
      ],
      sondes,
    );
    const agreement = forecastAgreement(compared);
    expect(agreement.cells).toBe(3);
    expect(agreement.above).toBe(2);
    expect(agreement.medianDeltaMhz).toBeCloseTo(2, 6);
    expect(agreement.widestAbove?.cell.key).toBe("IO92");
  });

  it("has nothing to say with nothing compared", () => {
    expect(forecastAgreement([])).toEqual({
      cells: 0,
      above: 0,
      medianDeltaMhz: 0,
      widestAbove: null,
    });
  });
});
