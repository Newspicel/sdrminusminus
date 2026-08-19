import { describe, expect, it } from "vitest";
import type { DfEstimate, DfGuidance } from "../types";
import {
  type BearingRay,
  destination,
  ellipseCollection,
  estimateCollection,
  navCollection,
  RAY_LENGTH_M,
  rayCollection,
  stationCollection,
} from "./df";

const HOME = { lat: 51.5, lon: 7.0 };

function ray(over: Partial<BearingRay> = {}): BearingRay {
  return { lat: HOME.lat, lon: HOME.lon, bearingDeg: 45, confidence: 0.9, ageMs: 0, ...over };
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
    const guidance: DfGuidance = {
      heading_deg: 135,
      mode: "cross",
      distance_m: 1_500,
      nav_target: { lat: 51.51, lon: 7.02, kind: "cross" },
    };
    const collection = navCollection(HOME, guidance);
    expect(collection.features[0]?.geometry.coordinates).toEqual([
      [7.0, 51.5],
      [7.02, 51.51],
    ]);
    expect(collection.features[0]?.properties.kind).toBe("cross");
    expect(navCollection(null, guidance).features).toHaveLength(0);
    expect(navCollection(HOME, null).features).toHaveLength(0);
  });
});
