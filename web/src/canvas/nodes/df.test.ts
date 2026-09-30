import { describe, expect, it } from "vitest";
import { RELATIVE_MARKS, spectrumPath, TRUE_MARKS } from "../../components/Rose";
import { isStale } from "../../lib/processors";
import type { ArrayGeometry, DfPeak, DfReading } from "../../lib/types";
import { allowsStructured } from "./arrayGeometry";
import {
  algorithmOptions,
  bearingLabel,
  dfChips,
  elevationBlock,
  frameOptions,
  frameRotation,
  NEEDS_HEADING,
  NEEDS_LINE_OR_CIRCLE,
  peakSummary,
  peakText,
  roseLetters,
  roseNeedles,
  roseWedge,
  shownFrame,
  sigmaText,
  smoothingTitle,
  sourceOptions,
  TOO_MANY_SOURCES,
} from "./df";

function peak(overrides: Partial<DfPeak> = {}): DfPeak {
  return {
    relative_deg: 47,
    true_deg: 137,
    power_db: -40,
    confidence: 0.82,
    sigma_deg: 3.1,
    ...overrides,
  };
}

function dfReading(overrides: Partial<DfReading> = {}): DfReading {
  return {
    at: "2026-09-28T12:00:00Z",
    peaks: [peak()],
    pseudospectrum: [],
    azimuth_deg: 90,
    station: { lat: 52, lon: 13 },
    sources: 1,
    sources_auto: true,
    squelched: false,
    aliasing: false,
    ...overrides,
  };
}

const SCATTERED: ArrayGeometry = {
  kind: "explicit",
  positions: [
    { x_m: 0, y_m: 0 },
    { x_m: 0.4, y_m: 0.1 },
    { x_m: 0.1, y_m: 0.5 },
  ],
};

const EVEN_ROW: ArrayGeometry = {
  kind: "explicit",
  positions: [
    { x_m: 0.8, y_m: 0.8 },
    { x_m: 0, y_m: 0 },
    { x_m: 0.4, y_m: 0.4 },
  ],
};

function disabled(geometry: ArrayGeometry): string[] {
  return algorithmOptions(allowsStructured(geometry, 3))
    .filter((option) => option.disabled === true)
    .map((option) => option.value);
}

describe("algorithmOptions", () => {
  it("offers root-MUSIC and ESPRIT only for a line or a circle", () => {
    expect(disabled({ kind: "ula", spacing_m: 0.4, axis_deg: 90 })).toEqual([]);
    expect(disabled({ kind: "uca", radius_m: 0.35 })).toEqual([]);
    expect(disabled(EVEN_ROW)).toEqual([]);
    expect(disabled(SCATTERED)).toEqual(["root_music", "esprit"]);
    const uneven = {
      ...EVEN_ROW,
      positions: [...EVEN_ROW.positions.slice(0, 2), { x_m: 0.2, y_m: 0.2 }],
    };
    expect(disabled(uneven)).toEqual(["root_music", "esprit"]);
    const refused = algorithmOptions(false).find((option) => option.value === "esprit");
    expect(refused?.title).toBe(NEEDS_LINE_OR_CIRCLE);
    expect(algorithmOptions(false).find((option) => option.value === "capon")?.disabled).toBe(
      false,
    );
    expect(smoothingTitle(false)).toBe(NEEDS_LINE_OR_CIRCLE);
    expect(smoothingTitle(true)).not.toBe(NEEDS_LINE_OR_CIRCLE);
  });

  it("counts sources up to one less than the lanes and keeps a stored count", () => {
    expect(sourceOptions(5).map((option) => option.label)).toEqual(["Auto", "1", "2", "3", "4"]);
    expect(sourceOptions(0).map((option) => option.label)).toEqual(["Auto", "1"]);
    expect(sourceOptions(16)).toHaveLength(16);
    const kept = sourceOptions(3, 4);
    expect(kept.map((option) => option.value)).toEqual([0, 1, 2, 3, 4]);
    expect(
      kept.filter((option) => option.title === TOO_MANY_SOURCES).map((option) => option.value),
    ).toEqual([3, 4]);
  });

  it("refuses elevation on a line and with a grid-free method or smoothing", () => {
    expect(elevationBlock(true, "music", 0)).toBe("Needs a 2D array");
    expect(elevationBlock(false, "esprit", 0)).toBe("Needs a grid method");
    expect(elevationBlock(false, "music", 2)).toBe("Needs a grid method");
    expect(elevationBlock(false, "capon", 0)).toBeNull();
  });
});

describe("bearing frame", () => {
  it("rotates the spectrum by the array azimuth", () => {
    const reading = dfReading({ azimuth_deg: 90 });
    expect(frameRotation(reading, "true")).toBe(90);
    expect(frameRotation(reading, "relative")).toBe(0);
    const spectrum = [255, 0, 0, 0];
    expect(spectrumPath(spectrum, 50, 10, 40, frameRotation(reading, "true"))).toMatch(
      /^M90\.00 50\.00/,
    );
    expect(spectrumPath(spectrum, 50, 10, 40, frameRotation(reading, "relative"))).toMatch(
      /^M50\.00 10\.00/,
    );
  });

  it("switches letters when the heading goes away", () => {
    expect(roseLetters(true)).toBe(TRUE_MARKS);
    expect(roseLetters(false)).toBe(RELATIVE_MARKS);
    expect(roseLetters(false).map((mark) => mark.label)).toEqual(["F", "R", "B", "L"]);
    expect(shownFrame(null, true)).toBe("true");
    expect(shownFrame("relative", true)).toBe("relative");
    expect(shownFrame("true", false)).toBe("relative");
    const unavailable = frameOptions(false).find((option) => option.value === "true");
    expect(unavailable).toMatchObject({ disabled: true, title: NEEDS_HEADING });
  });

  it("reads each peak in the chosen frame", () => {
    const first = peak({ mirror_deg: 313, mirror_true_deg: 43 });
    const second = peak({ relative_deg: 200, true_deg: 290, confidence: 0.4, sigma_deg: 0.42 });
    expect(peakText(first, "true")).toBe("137°");
    expect(peakText(first, "relative")).toBe("047°");
    expect(peakText(peak({ true_deg: null }), "true")).toBe("-");
    expect(roseNeedles([first, second], "true")).toEqual([
      { deg: 137, weight: "primary" },
      { deg: 43, weight: "mirror" },
      { deg: 290, weight: "secondary" },
    ]);
    expect(roseWedge([first], "relative")).toEqual({ deg: 47, sigmaDeg: 3.1 });
    expect(roseWedge([], "true")).toBeNull();
    expect(peakSummary(second, "true")).toBe("290° 40% ±0.4°");
    expect(sigmaText(3.1)).toBe("3°");
    expect(bearingLabel(359.7)).toBe("000°");
    expect(bearingLabel(-5)).toBe("355°");
  });
});

describe("dfChips", () => {
  it("lists chips for mirror, aliasing, squelch and stale", () => {
    const flagged = dfReading({ mirror: true, aliasing: true, squelched: true });
    expect(dfChips(flagged, null, true).map((chip) => chip.label)).toEqual([
      "Stale",
      "Squelch",
      "Aliased",
      "Mirror",
    ]);
    expect(dfChips(dfReading(), null, false)).toEqual([]);
    expect(dfChips(null, null, false)).toEqual([]);
  });

  it("has a chip for every flag and the array gate first", () => {
    const everything = dfReading({
      squelched: true,
      azimuth_deg: null,
      station: null,
      aliasing: true,
      mode_aliasing: true,
      mirror: true,
      rotating: true,
      singular: true,
      table_out_of_range: true,
    });
    const chips = dfChips(everything, "phase", false);
    expect(chips.map((chip) => chip.label)).toEqual([
      "Needs cal",
      "Squelch",
      "No heading",
      "No position",
      "Aliased",
      "Mode alias",
      "Mirror",
      "Rotating",
      "Singular",
      "Table off",
    ]);
    expect(chips.every((chip) => chip.title.length > 0)).toBe(true);
    expect(chips[0]?.danger).toBe(true);
  });

  it("calls a reading stale after three report periods", () => {
    expect(isStale(0, 2_999, 1_000)).toBe(false);
    expect(isStale(0, 3_001, 1_000)).toBe(true);
  });
});
