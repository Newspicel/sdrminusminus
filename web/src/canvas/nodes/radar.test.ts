import { describe, expect, it } from "vitest";
import type { RangeDopplerFrame } from "../../lib/frame";
import type { RadarProblem, RadarTrack } from "../../lib/types";
import { radarUpdate } from "../../test/fixtures";
import {
  bearingCell,
  cleaningOptions,
  clutterText,
  dopplerHzAt,
  frameAxes,
  hoverText,
  illuminatorEdit,
  isStale,
  lostCpis,
  problemLabel,
  pxToSurface,
  radarSubtitle,
  rangeKmAt,
  referenceText,
  sortTracks,
  surfaceToPx,
  surveillanceOf,
  toggledSurveillance,
  velocityMps,
  withReference,
} from "./radar";

function frame(overrides: Partial<RangeDopplerFrame> = {}): RangeDopplerFrame {
  return {
    streamId: 1,
    seq: 1,
    timestamp: 0n,
    ranges: 100,
    dopplers: 65,
    rangeFirstM: 0,
    rangeStepM: 150,
    dopplerFirstHz: -64,
    dopplerStepHz: 2,
    carrierHz: 100e6,
    dbMin: -3,
    dbMax: 30,
    cells: new Uint8Array(100 * 65),
    ...overrides,
  };
}

function track(id: number, snr: number, overrides: Partial<RadarTrack> = {}): RadarTrack {
  return {
    id,
    state: "confirmed",
    range_km: 10,
    range_rate_mps: -50,
    doppler_hz: 20,
    accel_mps2: 0,
    range_sigma_m: 10,
    rate_sigma_mps: 1,
    snr_db: snr,
    looks: 5,
    misses: 0,
    trail: [],
    ...overrides,
  };
}

describe("radar axes", () => {
  it("converts bins, rows and Doppler to km, Hz and m/s", () => {
    const axes = frameAxes(frame());
    expect(rangeKmAt(10, axes)).toBeCloseTo(1.5);
    expect(dopplerHzAt(0, axes)).toBe(-64);
    expect(dopplerHzAt(32, axes)).toBe(0);
    expect(velocityMps(10, 100e6)).toBeCloseTo(-29.98, 2);
    expect(velocityMps(10, 0)).toBeNull();
  });

  it("places a point on the plot", () => {
    const axes = frameAxes(frame());
    const box = { w: 400, h: 260 };
    expect(surfaceToPx({ rangeKm: 0, dopplerHz: 0 }, axes, box).y).toBeCloseTo(130);
    const far = surfaceToPx({ rangeKm: rangeKmAt(99, axes), dopplerHz: 64 }, axes, box);
    expect(far.x).toBeCloseTo(398);
    expect(far.y).toBeCloseTo(2);
    const back = pxToSurface(far.x, far.y, axes, box);
    expect(back.rangeKm).toBeCloseTo(rangeKmAt(99, axes));
    expect(back.dopplerHz).toBeCloseTo(64);
  });

  it("reads the hover point with sign and speed", () => {
    expect(hoverText({ rangeKm: 42.14, dopplerHz: -35 }, 100e6, 12.4)).toBe(
      "42.1 km · -35 Hz · +105 m/s · 12 dB",
    );
    expect(hoverText({ rangeKm: 1, dopplerHz: 3 }, 0, null)).toBe("1.0 km · +3 Hz");
  });
});

describe("radar tracks", () => {
  it("sorts tracks by SNR then id and keeps six", () => {
    const tracks = [
      track(4, 10),
      track(2, 20),
      track(1, 10),
      ...[5, 6, 7, 8].map((id) => track(id, 5)),
    ];
    const sorted = sortTracks(tracks, 6);
    expect(sorted.map((entry) => entry.id)).toEqual([2, 1, 4, 5, 6, 7]);
  });

  it("marks a bearing in the array frame when no heading is known", () => {
    expect(bearingCell(null)).toEqual({ text: "-", arrayFrame: false });
    expect(bearingCell({ azimuth_deg: 231.4, sigma_deg: 2, quality: 1 })).toEqual({
      text: "231°",
      arrayFrame: true,
    });
    expect(bearingCell({ azimuth_deg: 10, bearing_deg: 99.6, sigma_deg: 2, quality: 1 })).toEqual({
      text: "100°",
      arrayFrame: false,
    });
  });
});

describe("radar health", () => {
  it("counts every lost CPI", () => {
    const update = radarUpdate([]);
    const health = {
      ...update.health,
      dropped_cpis: 1,
      dropped_reports: 2,
      lagged_updates: 3,
      dropped_samples: 2_048_000,
    };
    expect(lostCpis(health, update.axes, 2_048_000)).toBe(6 + 4);
    expect(lostCpis(update.health, update.axes, null)).toBe(0);
  });

  it("calls a reading stale after three hops and a second", () => {
    const update = radarUpdate([]);
    expect(isStale(0, update, 1_750)).toBe(false);
    expect(isStale(0, update, 1_751)).toBe(true);
  });

  it("names the reference state and averages the clutter cut", () => {
    const update = radarUpdate([]);
    expect(referenceText(update)).toBe("Raw");
    const cma = { ...update.health.reference, mode: "cma" as const, quality_db: 6.2 };
    expect(referenceText({ ...update, health: { ...update.health, reference: cma } })).toBe(
      "CMA 6 dB",
    );
    const lost = { ...cma, locked: false };
    expect(referenceText({ ...update, health: { ...update.health, reference: lost } })).toBe(
      "Lost",
    );
    expect(clutterText([40, 42])).toEqual({ value: "41 dB", title: "L1 40 dB · L2 42 dB" });
  });

  it("keeps problem labels short", () => {
    const problems: RadarProblem[] = [
      { kind: "no_array" },
      { kind: "no_transmitter" },
      { kind: "no_receiver" },
      { kind: "no_heading" },
      { kind: "phase_unknown" },
      { kind: "overloaded" },
      { kind: "reference_lost" },
      { kind: "refused", detail: "CPI out of range" },
    ];
    for (const problem of problems) {
      const label = problemLabel(problem);
      expect(label.length).toBeGreaterThan(0);
      expect(label.length).toBeLessThanOrEqual(20);
      expect(label.includes(String.fromCharCode(0x2014))).toBe(false);
    }
    expect(problemLabel({ kind: "refused", detail: "CPI out of range" })).toBe("CPI out of range");
  });
});

describe("radarSubtitle", () => {
  const base = {
    wired: true,
    txWired: true,
    gate: null,
    update: null,
    stale: false,
    illuminator: "fm" as const,
    lanes: 5,
  };

  it("says no tx when the transmitter is unwired", () => {
    expect(radarSubtitle({ ...base, txWired: false }).text).toBe("no tx");
    expect(radarSubtitle({ ...base, wired: false }).text).toBe("no array");
    expect(radarSubtitle({ ...base, gate: "sync" }).text).toBe("syncing");
  });

  it("shows the first problem, staleness or the illuminator and carrier", () => {
    const update = radarUpdate([]);
    expect(radarSubtitle({ ...base, update }).text).toBe("FM 98.00 MHz");
    expect(radarSubtitle({ ...base, update, stale: true })).toEqual({ text: "stale", warn: true });
    const troubled = { ...update, problems: [{ kind: "no_receiver" as const }] };
    expect(radarSubtitle({ ...base, update: troubled }).text).toBe("No array position");
    expect(radarSubtitle(base).text).toBe("5 lanes");
  });
});

describe("radar settings edits", () => {
  it("toggles surveillance elements and folds back to all others", () => {
    const all = { kind: "all_others" as const };
    expect(surveillanceOf(all, 0, 5)).toEqual([1, 2, 3, 4]);
    const fewer = toggledSurveillance(all, 2, 0, 5);
    expect(fewer).toEqual({ kind: "mask", mask: 0b11010 });
    expect(toggledSurveillance(fewer, 2, 0, 5)).toEqual(all);
    expect(toggledSurveillance(all, 0, 0, 5)).toBe(all);
    const single = { kind: "mask" as const, mask: 0b10 };
    expect(toggledSurveillance(single, 1, 0, 5)).toBe(single);
  });

  it("keeps reference cleaning valid for the illuminator", () => {
    const cma = { kind: "cma" as const, taps: 16, step: 0.001 };
    expect(illuminatorEdit("dab", { illuminator: { kind: "fm" }, reference: cma })).toEqual({
      illuminator: { kind: "dab" },
      reference: { kind: "off" },
    });
    expect(
      illuminatorEdit("custom", { illuminator: { kind: "fm" }, reference: cma }).reference,
    ).toBe(cma);
    const offered = cleaningOptions("dab").map((option) => [option.value, option.disabled]);
    expect(offered).toEqual([
      ["off", false],
      ["cma", true],
      ["dab_remod", false],
    ]);
  });

  it("drops the new reference from the surveillance mask", () => {
    expect(withReference({ kind: "mask", mask: 0b110 }, 1)).toEqual({
      reference_element: 1,
      surveillance: { kind: "mask", mask: 0b100 },
    });
    expect(withReference({ kind: "mask", mask: 0b10 }, 1).surveillance).toEqual({
      kind: "all_others",
    });
  });
});
