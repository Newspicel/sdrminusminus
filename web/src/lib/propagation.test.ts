import { describe, expect, it } from "vitest";
import {
  alongGreatCircle,
  bearingDeg,
  controlPoints,
  decayWeight,
  EARTH_RADIUS_KM,
  eventSpot,
  greatCircleKm,
  gridToLatLon,
  hopCount,
  latLonToGrid,
  liveObservations,
  MIN_MUF_PATH_KM,
  maxHopKm,
  mergeObservations,
  messageCallsign,
  messageGrid,
  muf3000Mhz,
  obliquityFactor,
  observationOf,
  type PathObservation,
  propagationCells,
  propagationPaths,
  propagationSummary,
  receiverOf,
} from "./propagation";
import type { DecodedRecord } from "./types";

const NOW = Date.parse("2026-08-16T12:00:00Z");

const BERLIN: [number, number] = [52.5, 13];

function record(over: Partial<DecodedRecord> = {}): DecodedRecord {
  return {
    device_set: 0,
    channel: 0,
    at: new Date(NOW).toISOString(),
    freq_hz: 14_074_000,
    event: {
      kind: "ft8",
      data: { text: "CQ W1AW FN42", snr_db: -12, audio_hz: 1500, time_offset_s: 0, hard_errors: 0 },
    },
    ...over,
  };
}

function observation(over: Partial<PathObservation> = {}): PathObservation {
  return {
    key: "a",
    at: NOW,
    kind: "ft8",
    callsign: "W1AW",
    grid: "FN42",
    freqHz: 14_074_000,
    snrDb: -12,
    latitude: 42.5,
    longitude: -71,
    distanceKm: 6_000,
    bearingDeg: 290,
    hops: 2,
    muf3000Mhz: 16,
    control: [[50, -20]],
    ...over,
  };
}

describe("maidenhead locators", () => {
  it("puts a four-character square at its own centre", () => {
    expect(gridToLatLon("FN42")).toEqual([42.5, -71]);
    expect(gridToLatLon("JO62")).toEqual([52.5, 13]);
    expect(gridToLatLon("IO91")).toEqual([51.5, -1]);
  });

  it("reads lower case and the finer subsquares", () => {
    const coarse = gridToLatLon("JO62") ?? [0, 0];
    const fine = gridToLatLon("jo62qm") ?? [0, 0];
    expect(Math.abs(fine[0] - coarse[0])).toBeLessThan(0.5);
    expect(Math.abs(fine[1] - coarse[1])).toBeLessThan(1);
    expect(gridToLatLon("JO62QM12")).not.toBeNull();
  });

  it("refuses anything that is not a locator", () => {
    for (const bad of ["", "FN4", "FN421", "ZZ99", "F142", "FN4X", "JO62yy", "JO62QM1"]) {
      expect(gridToLatLon(bad), bad).toBeNull();
    }
  });

  it("round-trips a square through its centre", () => {
    for (const grid of ["FN42", "JO62", "IO91", "PM95", "AA00", "RR99"]) {
      const centre = gridToLatLon(grid);
      expect(centre, grid).not.toBeNull();
      expect(latLonToGrid(centre?.[0] ?? 0, centre?.[1] ?? 0)).toBe(grid);
    }
  });
});

describe("grids inside decoded messages", () => {
  it("takes the grid a CQ or a reply carries", () => {
    expect(messageGrid("CQ W1AW FN42")).toBe("FN42");
    expect(messageGrid("CQ DX JA1ABC PM95")).toBe("PM95");
    expect(messageGrid("W9XYZ W1AW FN42")).toBe("FN42");
    expect(messageGrid("cq w1aw fn42")).toBe("FN42");
  });

  it("does not read RR73 or a signal report as a square", () => {
    for (const text of [
      "W9XYZ W1AW RR73",
      "W9XYZ W1AW RRR",
      "W9XYZ W1AW 73",
      "W9XYZ W1AW -12",
      "W9XYZ W1AW R-12",
      "W9XYZ W1AW R+05",
      "TU; W9XYZ W1AW R 579 MA",
    ]) {
      expect(messageGrid(text), text).toBeNull();
    }
  });

  it("names the station the square belongs to", () => {
    expect(messageCallsign("CQ W1AW FN42")).toBe("W1AW");
    expect(messageCallsign("W9XYZ W1AW FN42")).toBe("W1AW");
    expect(messageCallsign("CQ <W1AW> FN42")).toBe("W1AW");
  });

  it("reads a WSPR spot's own grid field", () => {
    expect(
      eventSpot({
        kind: "wspr",
        data: {
          text: "K1ABC FN42 37",
          callsign: "K1ABC",
          grid: "FN42",
          power_dbm: 37,
          snr_db: -24,
          audio_hz: 1500,
          time_offset_s: 0,
          drift_hz: 0,
        },
      }),
    ).toEqual({ grid: "FN42", callsign: "K1ABC", snrDb: -24 });
  });

  it("has nothing to plot for a spot with no grid", () => {
    expect(
      eventSpot({
        kind: "wspr",
        data: {
          text: "K1ABC 37",
          callsign: "K1ABC",
          power_dbm: 37,
          snr_db: -24,
          audio_hz: 1500,
          time_offset_s: 0,
          drift_hz: 0,
        },
      }),
    ).toBeNull();
    expect(
      eventSpot({
        kind: "ft8",
        data: { text: "W9XYZ W1AW 73", snr_db: 0, audio_hz: 0, time_offset_s: 0, hard_errors: 0 },
      }),
    ).toBeNull();
  });
});

describe("great-circle geometry", () => {
  it("measures a quarter and a half of the equator", () => {
    expect(greatCircleKm([0, 0], [0, 90])).toBeCloseTo((Math.PI / 2) * EARTH_RADIUS_KM, 3);
    expect(greatCircleKm([0, 0], [0, 180])).toBeCloseTo(Math.PI * EARTH_RADIUS_KM, 3);
    expect(greatCircleKm([-90, 0], [90, 0])).toBeCloseTo(Math.PI * EARTH_RADIUS_KM, 3);
  });

  it("agrees with the published Berlin to Tokyo distance", () => {
    expect(greatCircleKm(BERLIN, [35.68, 139.77])).toBeGreaterThan(8_900);
    expect(greatCircleKm(BERLIN, [35.68, 139.77])).toBeLessThan(9_000);
  });

  it("points due north and due east along a meridian and the equator", () => {
    expect(bearingDeg([0, 0], [10, 0])).toBeCloseTo(0, 6);
    expect(bearingDeg([0, 0], [0, 10])).toBeCloseTo(90, 6);
    expect(bearingDeg([0, 0], [-10, 0])).toBeCloseTo(180, 6);
  });

  it("halves a path at its own midpoint", () => {
    const from: [number, number] = [0, 0];
    const to: [number, number] = [0, 80];
    expect(alongGreatCircle(from, to, 0.5)[1]).toBeCloseTo(40, 6);
    expect(alongGreatCircle(from, to, 0)[1]).toBeCloseTo(0, 6);
    expect(alongGreatCircle(from, from, 0.5)).toEqual([0, 0]);
  });
});

describe("the hop geometry behind a measured MUF", () => {
  it("reproduces the textbook M(3000) for the F2 layer", () => {
    expect(obliquityFactor(3_000, 300)).toBeCloseTo(3.2798, 3);
  });

  it("approaches one for a path that goes almost straight up", () => {
    expect(obliquityFactor(1, 300)).toBeCloseTo(1, 4);
  });

  it("splits a path longer than one hop can reach", () => {
    expect(maxHopKm(300)).toBeGreaterThan(3_800);
    expect(maxHopKm(300)).toBeLessThan(3_900);
    expect(hopCount(2_000, 300)).toBe(1);
    expect(hopCount(8_000, 300)).toBe(3);
    expect(hopCount(0, 300)).toBe(1);
  });

  it("leaves a 3000 km decode at its own frequency", () => {
    expect(muf3000Mhz(14_074_000, 3_000, 300)).toBeCloseTo(14.074, 6);
  });

  it("reads a short hop as a high MUF and a long one as barely above the band", () => {
    const short = muf3000Mhz(14_074_000, 1_000, 300) ?? 0;
    const long = muf3000Mhz(14_074_000, 8_000, 300) ?? 0;
    expect(short).toBeGreaterThan(24);
    expect(short).toBeLessThan(26);
    expect(long).toBeGreaterThan(14.074);
    expect(long).toBeLessThan(15.5);
  });

  it("refuses to invert a path too short to have gone through the ionosphere", () => {
    expect(muf3000Mhz(14_074_000, MIN_MUF_PATH_KM - 1, 300)).toBeNull();
    expect(muf3000Mhz(0, 3_000, 300)).toBeNull();
    expect(muf3000Mhz(Number.NaN, 3_000, 300)).toBeNull();
  });

  it("puts one control point at the midpoint and three on a three-hop path", () => {
    const single = controlPoints([0, 0], [0, 60], 1);
    expect(single).toHaveLength(1);
    expect(single[0]?.[1]).toBeCloseTo(30, 6);

    const triple = controlPoints([0, 0], [0, 60], 3);
    expect(triple).toHaveLength(3);
    expect(triple[0]?.[1]).toBeCloseTo(10, 6);
    expect(triple[2]?.[1]).toBeCloseTo(50, 6);
  });
});

describe("turning a decode into a path", () => {
  it("measures the path from the receiver to the transmitter's square", () => {
    const built = observationOf(record(), BERLIN, 300);
    expect(built).not.toBeNull();
    expect(built?.grid).toBe("FN42");
    expect(built?.callsign).toBe("W1AW");
    expect(built?.distanceKm).toBeGreaterThan(5_800);
    expect(built?.distanceKm).toBeLessThan(6_100);
    expect(built?.hops).toBe(2);
    expect(built?.control).toHaveLength(2);
    expect(built?.muf3000Mhz).not.toBeNull();
  });

  it("ignores a decode with no square and a kind that is not weak-signal", () => {
    expect(
      observationOf(
        record({
          event: {
            kind: "ft8",
            data: {
              text: "W9XYZ W1AW RR73",
              snr_db: 0,
              audio_hz: 0,
              time_offset_s: 0,
              hard_errors: 0,
            },
          },
        }),
        BERLIN,
        300,
      ),
    ).toBeNull();
    expect(
      observationOf(
        record({ event: { kind: "morse", data: { text: "CQ", wpm: 18 } } }),
        BERLIN,
        300,
      ),
    ).toBeNull();
  });
});

describe("decay and cells", () => {
  it("halves a weight over one half-life", () => {
    expect(decayWeight(0, 30)).toBe(1);
    expect(decayWeight(30 * 60_000, 30)).toBeCloseTo(0.5, 9);
    expect(decayWeight(60 * 60_000, 30)).toBeCloseTo(0.25, 9);
  });

  it("gathers reflection points into their own square and keeps the highest band", () => {
    const cells = propagationCells(
      [
        observation({ key: "a", control: [[50, -20]], freqHz: 14_074_000, muf3000Mhz: 16 }),
        observation({
          key: "b",
          control: [[50.4, -19.5]],
          freqHz: 21_074_000,
          muf3000Mhz: 24,
          callsign: "K1ABC",
        }),
        observation({ key: "c", control: [[10, 10]], freqHz: 7_074_000, muf3000Mhz: 9 }),
      ],
      { halfLifeMinutes: 30, nowMs: NOW },
    );
    expect(cells).toHaveLength(2);
    const busiest = cells[0];
    expect(busiest?.decodes).toBe(2);
    expect(busiest?.callsigns).toBe(2);
    expect(busiest?.bestFreqHz).toBe(21_074_000);
    expect(busiest?.measuredMuf3000Mhz).toBe(24);
    expect(busiest?.weight).toBeCloseTo(2, 6);
  });

  it("weighs an old decode less than a fresh one", () => {
    const cells = propagationCells(
      [
        observation({ key: "old", at: NOW - 60 * 60_000 }),
        observation({ key: "new", control: [[10, 10]] }),
      ],
      { halfLifeMinutes: 30, nowMs: NOW },
    );
    const [fresh, stale] = cells;
    expect(fresh?.weight).toBeCloseTo(1, 6);
    expect(stale?.weight).toBeCloseTo(0.25, 6);
  });

  it("leaves a cell without a usable MUF unmeasured", () => {
    const cells = propagationCells([observation({ muf3000Mhz: null })], {
      halfLifeMinutes: 30,
      nowMs: NOW,
    });
    expect(cells[0]?.measuredMuf3000Mhz).toBeNull();
  });
});

describe("paths, summary and retention", () => {
  it("draws one line per station and band, newest first", () => {
    const paths = propagationPaths(
      [
        observation({ key: "old", at: NOW - 1000 }),
        observation({ key: "new", at: NOW }),
        observation({ key: "other", grid: "JO62", at: NOW - 500 }),
      ],
      BERLIN,
      { halfLifeMinutes: 30, nowMs: NOW },
    );
    expect(paths).toHaveLength(2);
    expect(paths[0]?.key).toBe("new");
    expect(paths[0]?.from).toEqual([52.5, 13]);
  });

  it("counts what the station has actually heard", () => {
    const summary = propagationSummary([
      observation({ key: "a", freqHz: 14_074_000, muf3000Mhz: 16, distanceKm: 6_000 }),
      observation({
        key: "b",
        grid: "JO62",
        callsign: "DL1ABC",
        freqHz: 28_074_000,
        muf3000Mhz: 31,
        distanceKm: 400,
      }),
    ]);
    expect(summary.decodes).toBe(2);
    expect(summary.grids).toBe(2);
    expect(summary.callsigns).toBe(2);
    expect(summary.bands).toBe(2);
    expect(summary.bestFreqHz).toBe(28_074_000);
    expect(summary.bestMuf3000Mhz).toBe(31);
    expect(summary.farthestKm).toBe(6_000);
  });

  it("merges without duplicating, oldest first, and drops the overflow", () => {
    const held = mergeObservations([], [observation({ key: "a", at: NOW - 10 })]);
    const again = mergeObservations(held, [observation({ key: "a", at: NOW - 10 })]);
    expect(again).toBe(held);

    const merged = mergeObservations(held, [observation({ key: "b", at: NOW - 20 })]);
    expect(merged.map((entry) => entry.key)).toEqual(["b", "a"]);

    const capped = mergeObservations(
      [],
      [
        observation({ key: "x", at: NOW - 2 }),
        observation({ key: "y", at: NOW - 1 }),
        observation({ key: "z", at: NOW }),
      ],
      2,
    );
    expect(capped.map((entry) => entry.key)).toEqual(["y", "z"]);
  });

  it("stays cleared even when the stored history is merged back in", () => {
    const kept = liveObservations(
      [
        observation({ key: "before", at: NOW - 120_000 }),
        observation({ key: "after", at: NOW - 30_000 }),
      ],
      { halfLifeMinutes: 30, nowMs: NOW },
      NOW - 60_000,
    );
    expect(kept.map((entry) => entry.key)).toEqual(["after"]);
  });

  it("forgets what has decayed past keeping", () => {
    const kept = liveObservations(
      [
        observation({ key: "recent", at: NOW - 60_000 }),
        observation({ key: "ancient", at: NOW - 24 * 3_600_000 }),
      ],
      { halfLifeMinutes: 30, nowMs: NOW },
    );
    expect(kept.map((entry) => entry.key)).toEqual(["recent"]);
  });
});

describe("the receiver end of a path", () => {
  it("takes a fix and refuses one that is not a position", () => {
    expect(receiverOf({ latitude: 52.5, longitude: 13, time: "2026-08-16T12:00:00Z" })).toEqual([
      52.5, 13,
    ]);
    expect(receiverOf(undefined)).toBeNull();
    expect(
      receiverOf({ latitude: Number.NaN, longitude: 13, time: "2026-08-16T12:00:00Z" }),
    ).toBeNull();
  });
});
