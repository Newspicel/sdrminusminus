import { describe, expect, it } from "vitest";
import { BEARING_MAX_AGE_MS } from "./bearings";
import { crossingSourcesOf, dfOverlay } from "./dfOverlay";
import type { DfFusionState, PatchGraph } from "./types";

const HERE = { lat: 51.5, lon: 7.0 };

function graph(): PatchGraph {
  return {
    nodes: [
      { id: "cross", kind: "triangulation", data: {}, position: { x: 0, y: 0 } },
      { id: "nfm", kind: "channel", data: { channel_type: "nfm" }, position: { x: 0, y: 0 } },
      { id: "map", kind: "map", position: { x: 0, y: 0 } },
    ],
    edges: [
      { from: { node: "cross", port: "events" }, to: { node: "map", port: "events" } },
      { from: { node: "nfm", port: "events" }, to: { node: "map", port: "events" } },
    ],
  };
}

const FUSED: DfFusionState = {
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
  nav: {
    lat: 51.6,
    lon: 7.1,
    kind: "estimate",
    revision: 3,
    distance_m: 900,
    bearing_deg: 135,
  },
  stations: [{ station_id: "east", lat: 51.4, lon: 7.4, bearings: 2, last_seen: "now" }],
};

describe("crossingSourcesOf", () => {
  it("picks out only the triangulation nodes feeding a display", () => {
    expect(crossingSourcesOf(graph(), "map")).toEqual(["cross"]);
    expect(crossingSourcesOf(graph(), "nfm")).toEqual([]);
  });
});

describe("dfOverlay", () => {
  it("has nothing to draw with no triangulation wired in", () => {
    expect(dfOverlay([], {}, HERE)).toBeUndefined();
  });

  it("takes the estimate, nav target and stations from where the bearings cross", () => {
    const overlay = dfOverlay(["cross"], { cross: FUSED }, HERE);
    expect(overlay?.estimate?.converged).toBe(true);
    expect(overlay?.nav?.kind).toBe("estimate");
    expect(overlay?.stations).toHaveLength(1);
    expect(overlay?.rays).toEqual([]);
    expect(overlay?.maxAgeMs).toBe(BEARING_MAX_AGE_MS);
    expect(overlay?.from).toEqual(HERE);
  });

  it("draws an empty overlay while nothing has been fused", () => {
    const overlay = dfOverlay(["cross"], {}, null);
    expect(overlay?.estimate).toBeNull();
    expect(overlay?.stations).toEqual([]);
  });
});
