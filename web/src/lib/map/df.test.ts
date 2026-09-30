import { describe, expect, it } from "vitest";
import { greatCircleKm } from "../propagation";
import type { DfEstimate, NavTarget } from "../types";
import {
  advanceFixTrails,
  type BearingRay,
  bistaticCollection,
  bistaticRing,
  destination,
  ellipseCollection,
  estimateCollection,
  FIX_TRAIL_POINTS,
  fixEllipseCollection,
  navCollection,
  navTargetCollection,
  overlayCounts,
  RAY_LENGTH_M,
  type RadarFixPoint,
  rayCollection,
  siteCollection,
  stationCollection,
  trailCollection,
} from "./df";

const HOME = { lat: 51.5, lon: 7.0 };

function ray(over: Partial<BearingRay> = {}): BearingRay {
  return {
    lat: HOME.lat,
    lon: HOME.lon,
    bearingDeg: 45,
    confidence: 0.9,
    sigmaDeg: 3,
    ageMs: 0,
    ...over,
  };
}

function estimate(over: Partial<DfEstimate> = {}): DfEstimate {
  return {
    lat: 51.55,
    lon: 7.05,
    ellipse_major_m: 800,
    ellipse_minor_m: 200,
    ellipse_bearing_deg: 45,
    converged: false,
    samples: 8,
    ...over,
  };
}

describe("destination", () => {
  it("walks a bearing out to the distance it was given", () => {
    const [lon, lat] = destination(HOME.lat, HOME.lon, 0, 1_000);
    expect(lon).toBeCloseTo(HOME.lon, 6);
    expect(lat).toBeGreaterThan(HOME.lat);
    const [east] = destination(HOME.lat, HOME.lon, 90, 1_000);
    expect(east).toBeGreaterThan(HOME.lon);
  });
});

describe("rayCollection", () => {
  it("draws one line per bearing, out to the ray length", () => {
    const collection = rayCollection([ray()], 60_000);
    expect(collection.features).toHaveLength(1);
    const [start, end] = collection.features[0]?.geometry.coordinates ?? [];
    expect(start).toEqual([HOME.lon, HOME.lat]);
    expect(end).toEqual(destination(HOME.lat, HOME.lon, 45, RAY_LENGTH_M));
  });

  it("fades an older bearing and drops one past its age", () => {
    const fresh = rayCollection([ray({ ageMs: 0 })], 60_000).features[0]?.properties.weight ?? 0;
    const old = rayCollection([ray({ ageMs: 45_000 })], 60_000).features[0]?.properties.weight ?? 0;
    expect(old).toBeLessThan(fresh);
    expect(rayCollection([ray({ ageMs: 90_000 })], 60_000).features).toHaveLength(0);
  });
});

describe("estimateCollection", () => {
  it("marks nothing until there is an estimate", () => {
    expect(estimateCollection(null).features).toHaveLength(0);
    const marked = estimateCollection(estimate({ converged: true }));
    expect(marked.features[0]?.properties.converged).toBe(true);
    expect(marked.features[0]?.geometry.coordinates).toEqual([7.05, 51.55]);
  });
});

describe("ellipseCollection", () => {
  it("closes a ring longer along its major axis", () => {
    const collection = ellipseCollection(estimate());
    const ring = collection.features[0]?.geometry.coordinates[0] ?? [];
    expect(ring.length).toBeGreaterThan(8);
    expect(ring[0]).toEqual(ring[ring.length - 1]);
    const centre = estimate();
    const spans = ring.map(([lon, lat]) => Math.hypot(lon - centre.lon, lat - centre.lat));
    expect(Math.max(...spans)).toBeGreaterThan(Math.min(...spans) * 1.5);
  });

  it("has nothing to draw without an estimate", () => {
    expect(ellipseCollection(null).features).toHaveLength(0);
  });
});

describe("stationCollection", () => {
  it("places every station that has reported", () => {
    const collection = stationCollection([
      { station_id: "north", lat: 51.5, lon: 7.0, bearings: 3, last_seen: "now" },
    ]);
    expect(collection.features[0]?.properties.label).toBe("north");
  });
});

describe("navCollection", () => {
  it("draws the leg to the nav target and nothing without one", () => {
    const nav: NavTarget = {
      lat: 51.51,
      lon: 7.02,
      kind: "probe",
      revision: 1,
      distance_m: 1_500,
      bearing_deg: 135,
    };
    const collection = navCollection(HOME, nav);
    expect(collection.features[0]?.geometry.coordinates).toEqual([
      [7.0, 51.5],
      [7.02, 51.51],
    ]);
    expect(collection.features[0]?.properties.kind).toBe("probe");
    expect(navCollection(null, nav).features).toHaveLength(0);
    expect(navCollection(HOME, null).features).toHaveLength(0);
  });
});

describe("bistaticRing", () => {
  const set = {
    receiver: HOME,
    illuminator: { lat: 51.5, lon: 7.3 },
    rangesKm: [4],
  };

  it("puts every point where the echo's extra path adds up", () => {
    const ring = bistaticRing(set, 4) ?? [];
    expect(ring.length).toBeGreaterThan(8);
    const baselineKm = greatCircleKm(
      [set.receiver.lat, set.receiver.lon],
      [set.illuminator.lat, set.illuminator.lon],
    );
    for (const [lon, lat] of ring) {
      const total =
        greatCircleKm([set.receiver.lat, set.receiver.lon], [lat, lon]) +
        greatCircleKm([set.illuminator.lat, set.illuminator.lon], [lat, lon]);
      expect(total).toBeCloseTo(baselineKm + 4, 1);
    }
  });

  it("closes the ring and refuses an echo with no extra path", () => {
    const ring = bistaticRing(set, 4) ?? [];
    expect(ring[0]).toEqual(ring[ring.length - 1]);
    expect(bistaticRing(set, 0)).toBeNull();
  });

  it("draws one contour per echo and nothing without a transmitter to borrow", () => {
    expect(bistaticCollection([set]).features).toHaveLength(1);
    expect(bistaticCollection([]).features).toHaveLength(0);
    expect(bistaticCollection([{ ...set, rangesKm: [0] }]).features).toHaveLength(0);
  });
});

function fix(key: string, lat: number, lon: number): RadarFixPoint {
  return { key, id: 1, lat, lon, majorM: 300, minorM: 100, orientationDeg: 30 };
}

describe("radar marks", () => {
  it("marks the receiver and transmitter of every radar", () => {
    const collection = siteCollection([
      { node: "radar", receiver: { lat: 52, lon: 13 }, transmitter: { lat: 52.1, lon: 13.2 } },
    ]);
    expect(collection.features.map((feature) => feature.properties.role)).toEqual(["rx", "tx"]);
    expect(collection.features[1]?.geometry.coordinates).toEqual([13.2, 52.1]);
  });

  it("draws a one sigma ellipse around each fix", () => {
    const ring = fixEllipseCollection([fix("r:1", 52, 13)]).features[0]?.geometry.coordinates[0];
    expect(ring?.[0]).toEqual(ring?.at(-1));
    expect(fixEllipseCollection([{ ...fix("r:1", 52, 13), majorM: 0 }]).features).toEqual([]);
  });

  it("keeps a short trail of fixes and forgets ended tracks", () => {
    let trails: ReadonlyMap<string, readonly [number, number][]> = new Map();
    for (let step = 0; step < FIX_TRAIL_POINTS + 4; step++) {
      trails = advanceFixTrails(trails, [fix("r:1", 52 + step * 0.01, 13)]);
    }
    expect(trails.get("r:1")).toHaveLength(FIX_TRAIL_POINTS);
    const same = advanceFixTrails(trails, [fix("r:1", 52 + (FIX_TRAIL_POINTS + 3) * 0.01, 13)]);
    expect(same.get("r:1")).toBe(trails.get("r:1"));
    expect(advanceFixTrails(trails, []).size).toBe(0);
    expect(trailCollection(trails).features).toHaveLength(1);
  });

  it("marks the nav target", () => {
    const nav: NavTarget = {
      lat: 51.51,
      lon: 7.02,
      kind: "estimate",
      revision: 1,
      distance_m: 1_500,
      bearing_deg: 135,
    };
    expect(navTargetCollection(nav).features[0]?.geometry.coordinates).toEqual([7.02, 51.51]);
    expect(navTargetCollection(null).features).toEqual([]);
  });
});

describe("overlayCounts", () => {
  it("counts live rays, echoes and tracks and lists the unplaced", () => {
    const counts = overlayCounts({
      rays: [ray({ ageMs: 0 }), ray({ ageMs: 90_000 })],
      maxAgeMs: 60_000,
      estimate: null,
      emitters: [],
      nav: null,
      stations: [],
      bistatic: [{ receiver: HOME, illuminator: HOME, rangesKm: [3, 0, 5] }],
      sites: [],
      tracks: [fix("r:1", 52, 13)],
      unplaced: ["North DF"],
      from: null,
    });
    expect(counts).toEqual({ bearings: 1, echoes: 2, tracks: 1, unplaced: ["North DF"] });
    expect(overlayCounts(undefined).bearings).toBe(0);
  });

  it("draws the estimate and every other emitter", () => {
    const other = estimate({ lat: 51.7, lon: 7.2 });
    expect(ellipseCollection(estimate(), [estimate(), other]).features).toHaveLength(2);
  });
});
