import { describe, expect, it } from "vitest";
import type { PatchGraph, ProcessorStatus } from "../../lib/types";
import { arrayStatus, laneStatus, placed, radarUpdate } from "../../test/fixtures";
import { ageLabel, NO_ARRAY, processorGate, processorSubtitle, STALE } from "./processorFace";

const WIRED: PatchGraph = {
  nodes: [placed("arr", { kind: "array" }), placed("df", { kind: "df" })],
  edges: [{ from: { node: "arr", port: "array" }, to: { node: "df", port: "array" } }],
};

const UNWIRED: PatchGraph = { nodes: WIRED.nodes, edges: [] };

function gated(processor: string, gate: ProcessorStatus["gated"]): ProcessorStatus {
  return {
    node: processor,
    kind: "df",
    running: true,
    gated: gate,
    gated_samples: 0,
    dropped_samples: 0,
    dropped_reports: 0,
    lane_overflows: 0,
    lane_mismatch: 0,
    solver_failures: 0,
    resets: 0,
  };
}

const FIVE = arrayStatus("arr", { lanes: [0, 1, 2, 3, 4].map((lane) => laneStatus(lane)) });
const FRESH = {
  reading: { type: "passive_radar" as const, reading: radarUpdate([]) },
  receivedAt: 10_000,
};

describe("processorSubtitle", () => {
  it("says no array, the gate, stale or the lane count", () => {
    expect(processorSubtitle(UNWIRED, "df", FIVE, FRESH, 10_000, 500)).toBe(NO_ARRAY);
    const calibrating = { ...FIVE, processors: [gated("df", "calibrating")] };
    expect(processorSubtitle(WIRED, "df", calibrating, FRESH, 10_000, 500)).toBe("calibrating");
    const phase = { ...FIVE, processors: [gated("df", "phase")] };
    expect(processorSubtitle(WIRED, "df", phase, FRESH, 10_000, 500)).toBe("needs cal");
    expect(processorSubtitle(WIRED, "df", FIVE, FRESH, 12_001, 500)).toBe(STALE);
    expect(processorSubtitle(WIRED, "df", FIVE, FRESH, 11_000, 500)).toBe("5 lanes");
    expect(processorSubtitle(WIRED, "df", FIVE, undefined, 99_000, 500)).toBe("5 lanes");
  });

  it("reads the gate from the array's processor list only", () => {
    const other = { ...FIVE, processors: [gated("beam", "sync")] };
    expect(processorGate(other, "df")).toBeNull();
    expect(processorGate(other, "beam")).toBe("sync");
  });
});

describe("ageLabel", () => {
  it("formats ages", () => {
    expect(ageLabel(undefined, 1_000)).toBe("-");
    expect(ageLabel(1_000, 1_400)).toBe("0.4 s");
    expect(ageLabel(0, 12_300)).toBe("12 s");
    expect(ageLabel(0, 185_000)).toBe("3 min");
  });
});
