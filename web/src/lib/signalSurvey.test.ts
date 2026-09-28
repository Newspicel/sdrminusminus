import { describe, expect, it } from "vitest";
import {
  canStart,
  retunedSince,
  type SurveyView,
  signalOffsetLimitHz,
  signalSurveyCsv,
  surveyStatus,
} from "./signalSurvey";
import type { SurveyCell } from "./types";

function cell(overrides: Partial<SurveyCell> = {}): SurveyCell {
  return {
    latitude: 52.52,
    longitude: 13.405,
    frequency_hz: 145_500_000,
    level_dbfs: -67.125,
    measured_at: "2026-08-15T10:00:00Z",
    observations: 3,
    ...overrides,
  };
}

function view(overrides: Partial<SurveyView> = {}): SurveyView {
  return {
    radioWired: true,
    positionWired: true,
    positioned: true,
    targetHz: 145_500_000,
    levelDbfs: -60,
    recording: false,
    retuned: false,
    ...overrides,
  };
}

describe("signalOffsetLimitHz", () => {
  it("keeps the complete measurement width inside the IQ span", () => {
    expect(signalOffsetLimitHz(1_000_000, 12_500)).toBe(493_750);
    expect(signalOffsetLimitHz(10_000, 12_500)).toBe(0);
    expect(signalOffsetLimitHz(Number.NaN, 12_500)).toBe(0);
  });
});

describe("signalSurveyCsv", () => {
  it("exports server cells with their units and observation count", () => {
    const csv = signalSurveyCsv(
      [cell({ accuracy_m: 4.25 }), cell({ latitude: 52.53, level_dbfs: -70, observations: 1 })],
      -25_000,
      12_500,
    );
    const lines = csv.split("\n");
    expect(lines[0]).toBe(
      "time,frequency_hz,offset_hz,bandwidth_hz,latitude,longitude,accuracy_m,level_dbfs,observations",
    );
    expect(lines[1]).toBe(
      "2026-08-15T10:00:00Z,145500000,-25000,12500,52.5200000,13.4050000,4.3,-67.13,3",
    );
    expect(lines[2]).toBe(
      "2026-08-15T10:00:00Z,145500000,-25000,12500,52.5300000,13.4050000,,-70.00,1",
    );
  });

  it("writes only the header without cells", () => {
    expect(signalSurveyCsv([], 0, 12_500).split("\n")).toHaveLength(1);
  });
});

describe("the survey face state", () => {
  it("sees a retune only against the surveyed frequency", () => {
    expect(retunedSince([], 146_000_000)).toBe(false);
    expect(retunedSince([cell()], null)).toBe(false);
    expect(retunedSince([cell()], 145_500_000.2)).toBe(false);
    expect(retunedSince([cell()], 146_000_000)).toBe(true);
  });

  it("starts only when wired, placed, measuring and on the surveyed frequency", () => {
    expect(canStart(view())).toBe(true);
    expect(canStart(view({ radioWired: false }))).toBe(false);
    expect(canStart(view({ positionWired: false }))).toBe(false);
    expect(canStart(view({ positioned: false }))).toBe(false);
    expect(canStart(view({ levelDbfs: null }))).toBe(false);
    expect(canStart(view({ retuned: true }))).toBe(false);
  });

  it("says what the survey waits for", () => {
    expect(surveyStatus(view({ radioWired: false }))).toBe("Wire a radio");
    expect(surveyStatus(view({ positionWired: false }))).toBe("Wire a position");
    expect(surveyStatus(view({ recording: true }))).toBe("Recording each new GPS fix");
    expect(surveyStatus(view({ retuned: true }))).toBe("Retuned: clear to start again");
    expect(surveyStatus(view({ retuned: true, recording: true }))).toBe(
      "Recording each new GPS fix",
    );
    expect(surveyStatus(view({ targetHz: null, levelDbfs: null }))).toBe("Waiting for the radio");
    expect(surveyStatus(view({ levelDbfs: null }))).toBe("Offset is outside the IQ span");
    expect(surveyStatus(view({ positioned: false }))).toBe("Waiting for GPS");
    expect(surveyStatus(view())).toBe("Ready");
  });
});
