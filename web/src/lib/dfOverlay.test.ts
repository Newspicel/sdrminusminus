import { describe, expect, it } from "vitest";
import { DEFAULT_RADAR_PARAMS } from "../canvas/nodes/radar";
import type { DfNodeState } from "./df";
import { BEARING_MAX_AGE_MS, dfOverlay, dfSourcesOf, radarSourcesOf } from "./dfOverlay";
import type { PatchGraph } from "./types";

const HERE = { lat: 51.5, lon: 7.0 };

function graph(): PatchGraph {
  return {
    nodes: [
      { id: "df", kind: "df", data: {}, position: { x: 0, y: 0 } },
      { id: "nfm", kind: "channel", data: { channel_type: "nfm" }, position: { x: 0, y: 0 } },
      { id: "map", kind: "map", position: { x: 0, y: 0 } },
    ],
    edges: [
      { from: { node: "df", port: "events" }, to: { node: "map", port: "events" } },
      { from: { node: "nfm", port: "events" }, to: { node: "map", port: "events" } },
    ],
  };
}

function node(over: Partial<DfNodeState> = {}): DfNodeState {
  return {
    deviceSet: 1,
    reading: { bearing_deg: 45, confidence: 0.8, peak_to_floor_db: 20, pseudospectrum: [] },
    cal: { tier: "phase_coherent", lanes: [], phase_unknown: false, solved: true },
    at: 1_000,
    history: [{ bearingDeg: 45, confidence: 0.8, at: 1_000 }],
    ...over,
  };
}

describe("dfSourcesOf", () => {
  it("picks out only the direction finders feeding a display", () => {
    expect(dfSourcesOf(graph(), "map")).toEqual(["df"]);
    expect(dfSourcesOf(graph(), "nfm")).toEqual([]);
  });
});

describe("dfOverlay", () => {
  it("has nothing to draw with no finders wired in", () => {
    expect(dfOverlay([], {}, 1_000, HERE)).toBeUndefined();
  });

  it("turns the trail into rays measured from where the station stood", () => {
    const overlay = dfOverlay(["df"], { df: node() }, 3_000, HERE);
    expect(overlay?.rays).toEqual([
      { lat: HERE.lat, lon: HERE.lon, bearingDeg: 45, confidence: 0.8, ageMs: 2_000 },
    ]);
    expect(overlay?.maxAgeMs).toBe(BEARING_MAX_AGE_MS);
    expect(overlay?.from).toEqual(HERE);
  });

  it("prefers the place a sample was taken over where the station is now", () => {
    const state = node({
      history: [{ bearingDeg: 90, confidence: 0.5, at: 1_000, lat: 52, lon: 8 }],
    });
    expect(dfOverlay(["df"], { df: state }, 1_000, HERE)?.rays[0]).toMatchObject({
      lat: 52,
      lon: 8,
    });
  });

  it("drops a bearing that has nowhere to be drawn from", () => {
    expect(dfOverlay(["df"], { df: node() }, 1_000, null)?.rays).toEqual([]);
  });

  it("carries the fused estimate, guidance and stations through", () => {
    const state = node({
      fusion: {
        samples: 4,
        estimate: {
          lat: 51.6,
          lon: 7.1,
          ellipse_major_m: 300,
          ellipse_minor_m: 200,
          ellipse_bearing_deg: 10,
          converged: true,
          samples: 4,
        },
        guidance: {
          heading_deg: 135,
          mode: "approach",
          distance_m: 900,
          nav_target: { lat: 51.6, lon: 7.1, kind: "target" },
        },
        stations: [{ station_id: "east", lat: 51.4, lon: 7.4, bearings: 2, last_seen: "now" }],
      },
    });
    const overlay = dfOverlay(["df"], { df: state }, 1_000, HERE);
    expect(overlay?.estimate?.converged).toBe(true);
    expect(overlay?.guidance?.mode).toBe("approach");
    expect(overlay?.stations).toHaveLength(1);
  });
});

function radarGraph(illuminator: { lat: number; lon: number; freq_hz: number } | null): PatchGraph {
  return {
    nodes: [
      {
        id: "radar",
        kind: "passive_radar",
        data: { settings: { ...DEFAULT_RADAR_PARAMS, illuminator } },
        position: { x: 0, y: 0 },
      },
      { id: "map", kind: "map", position: { x: 0, y: 0 } },
    ],
    edges: [{ from: { node: "radar", port: "events" }, to: { node: "map", port: "events" } }],
  };
}

describe("radarSourcesOf", () => {
  it("takes only the radars that say where they borrow their transmitter from", () => {
    const found = radarSourcesOf(radarGraph({ lat: 51.5, lon: 7.3, freq_hz: 100e6 }), "map");
    expect(found).toEqual([{ node: "radar", illuminator: { lat: 51.5, lon: 7.3 } }]);
    expect(radarSourcesOf(radarGraph(null), "map")).toEqual([]);
  });
});

describe("dfOverlay echoes", () => {
  const radars = [{ node: "radar", illuminator: { lat: 51.5, lon: 7.3 } }];
  const detections = {
    radar: {
      ...node(),
      detections: [
        { range_bin: 12, range_km: 3.6, doppler_hz: 40, snr_db: 12 },
        { range_bin: 30, range_km: 9, doppler_hz: -20, snr_db: 9 },
      ],
    },
  };

  it("hands the map one contour set per radar that has echoes", () => {
    const overlay = dfOverlay([], detections, 1_000, HERE, radars);
    expect(overlay?.bistatic).toEqual([
      { receiver: HERE, illuminator: radars[0]?.illuminator, rangesKm: [3.6, 9] },
    ]);
  });

  it("draws no ellipse until the receiver knows where it is", () => {
    expect(dfOverlay([], detections, 1_000, null, radars)?.bistatic).toEqual([]);
    expect(dfOverlay([], { radar: node() }, 1_000, HERE, radars)?.bistatic).toEqual([]);
  });
});
