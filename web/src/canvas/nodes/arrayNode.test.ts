import { describe, expect, it } from "vitest";
import type { PatchEdge, PatchGraph } from "../../lib/types";
import { catalogBody } from "../../test/catalog";
import { arrayStatus, capabilities, deviceSet, laneStatus, placed } from "../../test/fixtures";
import { streamPort } from "../graph";
import {
  arrayLaneRows,
  arraySpanHz,
  arraySubtitle,
  calLabel,
  calSourceOf,
  calTitle,
  checkOptions,
  delayLabel,
  failureTitle,
  headingLabel,
  heldLanes,
  laneGaps,
  laneQualityPercent,
  laneTitle,
  memberDevices,
  NO_LANES,
  PICK_ONE,
  recordingLabel,
  switchedOrientation,
  syncLabel,
  TIER_OPTIONS,
  TIER_TEXT,
  tierLabel,
  tierOptions,
} from "./arrayNode";

function lane(device: string, stream: number, array = "north", slot = stream): PatchEdge {
  return {
    from: { node: device, port: streamPort("iq", stream) },
    to: { node: array, port: streamPort("lane", slot) },
  };
}

function patch(edges: PatchEdge[]): PatchGraph {
  return {
    nodes: [
      placed("kraken", {
        kind: "device",
        data: { device: { backend: "virtual", key: "kraken5" } },
      }),
      placed("rtl", { kind: "device", data: { device: { backend: "rtlsdr", serial: "7" } } }),
      placed("north", { ...catalogBody("array"), label: "North" }),
      placed("south", catalogBody("array")),
    ],
    edges,
  };
}

function present<T>(value: T | undefined): T {
  if (value === undefined) {
    throw new Error("missing row");
  }
  return value;
}

const KRAKEN = deviceSet({
  capabilities: capabilities({ rx_streams: 5, per_stream: { tuning: true } }),
});

describe("arrayLaneRows", () => {
  it("labels lanes by port, not by position in the list", () => {
    const graph = patch([lane("kraken", 0), lane("kraken", 2)]);
    const rows = arrayLaneRows(graph, new Map(), "north", undefined);
    expect(rows.map((row) => [row.lane, row.port])).toEqual([
      [1, "lane"],
      [3, "lane3"],
    ]);
  });

  it("names each lane by radio and stream", () => {
    const graph = patch([lane("kraken", 0), lane("kraken", 2), lane("rtl", 0, "north", 3)]);
    const status = arrayStatus("north", {
      lanes: [laneStatus(0), laneStatus(2, { phase_deg: 12.34 }), laneStatus(3)],
    });
    const rows = arrayLaneRows(graph, new Map([["kraken", KRAKEN]]), "north", status);
    expect(rows.map((row) => row.sourceLabel)).toEqual([
      "KrakenSDR iq1",
      "KrakenSDR iq3",
      "rtlsdr · 7 iq",
    ]);
    expect(rows[1]?.status?.phase_deg).toBe(12.34);
    expect(laneTitle(present(rows[1]))).toBe(
      "Lane 3 · KrakenSDR iq3 · phase 12.34° · gain 0.00 dB · delay 0.000 smp · gaps 0",
    );
    const unmeasured = arrayLaneRows(graph, new Map(), "north", undefined)[0];
    expect(unmeasured?.status).toBeNull();
    expect(laneTitle(present(unmeasured))).toBe("Lane 1 · virtual · kraken5 iq");
    expect(memberDevices(graph, "north")).toEqual(["kraken", "rtl"]);
  });

  it("maps held device streams to their array", () => {
    const graph = patch([lane("kraken", 0), lane("kraken", 1), lane("kraken", 4, "south", 0)]);
    expect([...heldLanes(graph, "kraken")]).toEqual([
      [0, "north"],
      [1, "north"],
      [4, "south"],
    ]);
    expect(heldLanes(graph, "rtl").size).toBe(0);
  });
});

describe("array labels", () => {
  it("labels tiers truthfully", () => {
    expect(TIER_TEXT.none).toBe("None");
    expect(tierLabel(arrayStatus("north", { tier: "none" }))).toBe("None");
    expect(tierLabel(arrayStatus("north", { tier: "time_sync", tier_capped: true }))).toBe(
      "Shared clock · capped",
    );
    expect(tierLabel(arrayStatus("north", { tier: "phase_coherent" }))).toBe("Shared LO");
  });

  it("says how long ago calibration solved", () => {
    const solvedAt = Date.parse("2026-09-28T12:00:00Z");
    const solved = arrayStatus("north", { cal: "solved", last_solve_at: "2026-09-28T12:00:00Z" });
    expect(calLabel(solved, solvedAt + 12_300)).toBe("Calibrated 12 s ago");
    expect(calLabel(solved, solvedAt + 185_000)).toBe("Calibrated 3 min ago");
    expect(calLabel(arrayStatus("north", { cal: "none" }), solvedAt)).toBe("No cal");
    expect(calLabel(arrayStatus("north", { cal: "failed" }), solvedAt)).toBe("Cal failed");
    expect(calLabel(arrayStatus("north", { cal: "measuring" }), solvedAt)).toBe("Calibrating");
  });

  it("titles calibration with the failure or the next check", () => {
    const failed = arrayStatus("north", { cal: "failed", failure: { kind: "noise_not_seen" } });
    expect(calTitle(failed)).toBe("Noise not seen");
    expect(calTitle(arrayStatus("north", { next_check_in_s: 39.6 }))).toBe("Next check in 40 s");
    expect(calTitle(arrayStatus("north"))).toBeUndefined();
  });

  it("adds clock drift to the sync state", () => {
    expect(syncLabel(arrayStatus("north", { sync: "locked", drift_ppm: 0.24 }))).toBe(
      "Locked · 0.2 ppm",
    );
    expect(syncLabel(arrayStatus("north", { sync: "searching" }))).toBe("Syncing");
  });

  it("titles the header by lanes, failure, then sync", () => {
    expect(arraySubtitle(undefined, 0)).toBe(NO_LANES);
    expect(arraySubtitle(undefined, 2)).toBe("idle");
    const failed = arrayStatus("north", { failure: { kind: "unwired" } });
    expect(arraySubtitle(failed, 3)).toBe("failed");
    expect(arraySubtitle(arrayStatus("north", { sync: "drifting" }), 3)).toBe("drifting");
    expect(failureTitle({ kind: "stopped", message: "USB gone" })).toBe("USB gone");
    expect(failureTitle({ kind: "lane_gap", lane: 1 })).toBe("Lane 2 unwired");
  });

  it("names the heading by orientation and source", () => {
    const heading = { kind: "heading" as const, mount_offset_deg: 0 };
    expect(headingLabel({ kind: "fixed", azimuth_deg: 90 }, undefined)).toBe("Fixed 90°");
    expect(headingLabel(heading, undefined)).toBe("None");
    const fixed = arrayStatus("north", { azimuth_deg: 123.4, heading_source: "compass" });
    expect(headingLabel(heading, fixed)).toBe("123° compass");
    expect(headingLabel(heading, arrayStatus("north", { azimuth_deg: -0.2 }))).toBe("0°");
  });

  it("keeps quality inside the bar and a stored check interval in the list", () => {
    expect(laneQualityPercent(0.826)).toBe(83);
    expect(laneQualityPercent(1.4)).toBe(100);
    expect(laneQualityPercent(-1)).toBe(0);
    expect(checkOptions(60).map((option) => option.label)).toEqual([
      "Off",
      "10 s",
      "60 s",
      "5 min",
    ]);
    expect(checkOptions(30).map((option) => option.value)).toEqual([0, 10, 30, 60, 300]);
  });
});

describe("array settings", () => {
  it("offers a tier to pick when none is stored", () => {
    expect(tierOptions("time_sync")).toBe(TIER_OPTIONS);
    const unset = tierOptions("none");
    expect(unset[0]).toMatchObject({ value: "none", label: PICK_ONE, disabled: true });
    expect(unset.slice(1)).toEqual(TIER_OPTIONS);
  });

  it("keeps offset and width when the calibration source changes", () => {
    const pilot = calSourceOf("pilot", { kind: "noise" });
    expect(pilot).toEqual({ kind: "pilot", offset_hz: 0, bandwidth_hz: 20_000 });
    const tuned = { kind: "pilot" as const, offset_hz: 12_500, bandwidth_hz: 5_000 };
    expect(calSourceOf("emitter", tuned)).toEqual({
      kind: "emitter",
      offset_hz: 12_500,
      bandwidth_hz: 5_000,
      bearing_deg: 0,
    });
    const emitter = { ...tuned, kind: "emitter" as const, bearing_deg: 42 };
    expect(calSourceOf("emitter", emitter)).toEqual(emitter);
    expect(calSourceOf("pilot", emitter)).toEqual(tuned);
    expect(calSourceOf("off", emitter)).toEqual({ kind: "off" });
  });

  it("starts a fixed array where the heading last pointed", () => {
    const fixed = { kind: "fixed" as const, azimuth_deg: 45 };
    expect(switchedOrientation("fixed", fixed, 200)).toBe(fixed);
    expect(switchedOrientation("heading", fixed, 200)).toEqual({
      kind: "heading",
      mount_offset_deg: 0,
    });
    const heading = { kind: "heading" as const, mount_offset_deg: 10 };
    expect(switchedOrientation("fixed", heading, 359.7)).toEqual({ kind: "fixed", azimuth_deg: 0 });
    expect(switchedOrientation("fixed", heading, -91.2)).toEqual({
      kind: "fixed",
      azimuth_deg: 269,
    });
    expect(switchedOrientation("fixed", heading, null)).toEqual({ kind: "fixed", azimuth_deg: 0 });
  });
});

describe("array readouts", () => {
  it("says how long a recording runs and what it dropped", () => {
    const running = { stem: "north-1", started_at: "", samples: 4_096_000, dropped: 0 };
    expect(recordingLabel(running, 2_048_000)).toBe("2 s");
    expect(recordingLabel({ ...running, dropped: 12 }, 2_048_000)).toBe("2 s · 12 dropped");
    expect(recordingLabel({ ...running, error: "Disk full" }, 2_048_000)).toBe("Disk full");
  });

  it("keeps a lane delay within six characters", () => {
    expect(delayLabel(0)).toBe("0.00");
    expect(delayLabel(-4.567)).toBe("-4.57");
    expect(delayLabel(-123.45)).toBe("-123.5");
    expect(delayLabel(16_803.25)).toBe("16803");
    expect(delayLabel(-99_999.4)).toBe("-99999");
    expect(delayLabel(1_234_567)).toBe("1235k");
    expect(delayLabel(-999.96)).toBe("-1000");
    expect(delayLabel(-99_999.6)).toBe("-100k");
    for (const delay of [-9.999, -999.96, -16_803.25, -99_999.6, -480_000, -1_234_567]) {
      expect(delayLabel(delay).length).toBeLessThanOrEqual(6);
    }
  });

  it("adds up lane gaps", () => {
    const status = arrayStatus("north", {
      lanes: [laneStatus(0, { gaps: 2 }), laneStatus(1), laneStatus(2, { gaps: 5 })],
    });
    expect(laneGaps(status)).toBe(7);
  });

  it("spans from the lowest to the highest held lane plus one rate", () => {
    const spread = deviceSet({
      capabilities: capabilities({ rx_streams: 3, per_stream: { tuning: true } }),
      settings: {
        center_hz: 100_000_000,
        streams: [
          { stream: 1, center_hz: 102_000_000 },
          { stream: 2, center_hz: 104_000_000 },
        ],
      },
    });
    const graph = patch([lane("kraken", 0), lane("kraken", 1), lane("kraken", 2)]);
    const status = arrayStatus("north", { sample_rate: 2_400_000 });
    expect(arraySpanHz(graph, new Map([["kraken", spread]]), "north", status)).toBe(6_400_000);
    expect(arraySpanHz(graph, new Map(), "north", status)).toBeNull();
    expect(arraySpanHz(graph, new Map([["kraken", spread]]), "north", undefined)).toBeNull();
  });
});
