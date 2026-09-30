import { describe, expect, it } from "vitest";
import { placed, radarUpdate } from "../test/fixtures";
import { BEARING_MAX_AGE_MS, type BearingSample } from "./bearings";
import { dfOverlay, type OverlaySources, overlaySourcesOf } from "./dfOverlay";
import { bistaticRing } from "./map/df";
import { type ProcessorState, readingOf } from "./processors";
import type { DfFusionState, DfReading, PatchGraph, RadarTrack, RadarUpdate } from "./types";

const NOW = 1_000_000;
const HERE = { lat: 51.5, lon: 7.0 };

function graph(): PatchGraph {
  return {
    nodes: [
      placed("finder", { kind: "df", label: "North DF" }),
      placed("cross", { kind: "triangulation" }),
      placed("radar", { kind: "passive_radar" }),
      placed("filter", { kind: "event_filter" }),
      placed("hunt", { kind: "hunt" }),
      placed("nfm", { kind: "channel", data: { channel_type: "nfm" } }),
      placed("map", { kind: "map" }),
    ],
    edges: [
      { from: { node: "finder", port: "events" }, to: { node: "map", port: "events" } },
      { from: { node: "cross", port: "events" }, to: { node: "map", port: "events" } },
      { from: { node: "radar", port: "events" }, to: { node: "map", port: "events" } },
      { from: { node: "hunt", port: "events" }, to: { node: "filter", port: "events" } },
      { from: { node: "filter", port: "events" }, to: { node: "map", port: "events" } },
      { from: { node: "nfm", port: "events" }, to: { node: "map", port: "events" } },
    ],
  };
}

function sources(overrides: Partial<OverlaySources> = {}): OverlaySources {
  return {
    finders: [],
    hunts: [],
    crossings: [],
    radars: [],
    labels: { finder: "North DF", radar: "Passive radar" },
    ...overrides,
  };
}

function sample(node: string, lat: number, lon: number, trueDeg: number): BearingSample {
  return {
    node,
    trueDeg,
    confidence: 0.8,
    sigmaDeg: 3,
    lat,
    lon,
    at: NOW - 1_000,
    stationId: null,
  };
}

function dfState(station: boolean): ProcessorState {
  const reading: DfReading = {
    at: "now",
    peaks: [
      {
        relative_deg: 10,
        true_deg: station ? 100 : null,
        power_db: -40,
        confidence: 0.9,
        sigma_deg: 2,
      },
    ],
    pseudospectrum: [],
    sources: 1,
    sources_auto: true,
    squelched: false,
    aliasing: false,
    station: station ? { lat: 52, lon: 13 } : null,
    azimuth_deg: station ? 0 : null,
  };
  return { reading: { type: "df", reading }, receivedAt: NOW };
}

function track(id: number, rangeKm: number, overrides: Partial<RadarTrack> = {}): RadarTrack {
  return {
    id,
    state: "confirmed",
    range_km: rangeKm,
    range_rate_mps: -50,
    doppler_hz: 20,
    accel_mps2: 0,
    range_sigma_m: 10,
    rate_sigma_mps: 1,
    snr_db: 12,
    looks: 5,
    misses: 0,
    trail: [],
    ...overrides,
  };
}

function radarState(update: RadarUpdate, receivedAt = NOW): ProcessorState {
  return { reading: { type: "passive_radar", reading: update }, receivedAt };
}

const GEOMETRY = {
  receiver: { lat: 52.0, lon: 13.0, altitude_m: 40 },
  transmitter: { lat: 52.1, lon: 13.2, altitude_m: 200 },
  baseline_km: 17,
};

describe("overlaySourcesOf", () => {
  it("sorts the map's event wires into finders, hunts, crossings and radars", () => {
    const found = overlaySourcesOf(graph(), "map");
    expect(found.finders).toEqual(["finder"]);
    expect(found.hunts).toEqual(["hunt"]);
    expect(found.crossings).toEqual(["cross"]);
    expect(found.radars).toEqual(["radar"]);
    expect(found.labels.finder).toBe("North DF");
    expect(found.labels.radar).toBe("Passive radar");
  });
});

describe("dfOverlay", () => {
  it("has nothing to draw with nothing wired in", () => {
    expect(dfOverlay(sources(), {}, {}, {}, NOW, HERE)).toBeUndefined();
  });

  it("draws each ray from where its bearing was taken", () => {
    const overlay = dfOverlay(
      sources({ finders: ["finder"], hunts: ["hunt"] }),
      {
        finder: [sample("finder", 52, 13, 45), sample("finder", 52.1, 13.1, 50)],
        hunt: [sample("hunt", 51, 12, 270)],
      },
      {},
      {},
      NOW,
      HERE,
    );
    expect(overlay?.rays.map((ray) => [ray.lat, ray.lon, ray.bearingDeg])).toEqual([
      [52, 13, 45],
      [52.1, 13.1, 50],
      [51, 12, 270],
    ]);
    expect(overlay?.rays[0]?.ageMs).toBe(1_000);
    expect(overlay?.maxAgeMs).toBe(BEARING_MAX_AGE_MS);
  });

  it("draws nothing from the map's own position", () => {
    const overlay = dfOverlay(sources({ finders: ["finder"] }), {}, {}, {}, NOW, HERE);
    expect(overlay?.rays).toEqual([]);
    expect(overlay?.sites).toEqual([]);
    expect(overlay?.from).toEqual(HERE);
  });

  it("centres bistatic rings on the radar's receiver and transmitter", () => {
    const update = {
      ...radarUpdate([]),
      geometry: GEOMETRY,
      tracks: [
        track(1, 12, {
          fix: {
            lat: 52.05,
            lon: 13.1,
            alt_m: 8000,
            major_m: 300,
            minor_m: 100,
            orientation_deg: 30,
          },
        }),
        track(2, 20, { state: "coasting" }),
      ],
    };
    const overlay = dfOverlay(
      sources({ radars: ["radar"] }),
      {},
      {},
      { radar: radarState(update) },
      NOW,
      HERE,
    );
    const echoes = overlay?.bistatic[0];
    expect(echoes?.receiver).toEqual({ lat: 52.0, lon: 13.0 });
    expect(echoes?.illuminator).toEqual({ lat: 52.1, lon: 13.2 });
    expect(echoes?.rangesKm).toEqual([12]);
    expect(overlay?.tracks).toEqual([
      {
        key: "radar:1",
        id: 1,
        lat: 52.05,
        lon: 13.1,
        majorM: 300,
        minorM: 100,
        orientationDeg: 30,
      },
    ]);
    expect(overlay?.sites).toEqual([
      { node: "radar", receiver: { lat: 52, lon: 13 }, transmitter: { lat: 52.1, lon: 13.2 } },
    ]);
    const ring = echoes === undefined ? null : bistaticRing(echoes, 12);
    expect(
      ring?.some(([lon, lat]) => Math.abs(lat - HERE.lat) < 0.2 && Math.abs(lon - HERE.lon) < 0.2),
    ).toBe(false);
  });

  it("stops drawing echoes from a stale radar", () => {
    const update = { ...radarUpdate([]), geometry: GEOMETRY, tracks: [track(1, 12)] };
    const overlay = dfOverlay(
      sources({ radars: ["radar"] }),
      {},
      {},
      { radar: radarState(update, NOW - 60_000) },
      NOW,
      HERE,
    );
    expect(overlay?.bistatic).toEqual([]);
    expect(overlay?.sites).toHaveLength(1);
  });

  it("lists sources with no position as unplaced", () => {
    const overlay = dfOverlay(
      sources({ finders: ["finder"], radars: ["radar"] }),
      {},
      {},
      { finder: dfState(false), radar: radarState(radarUpdate([])) },
      NOW,
      null,
    );
    expect(overlay?.unplaced).toEqual(["North DF", "Passive radar"]);
    const placedOverlay = dfOverlay(
      sources({ finders: ["finder"] }),
      {},
      {},
      { finder: dfState(true) },
      NOW,
      null,
    );
    expect(placedOverlay?.unplaced).toEqual([]);
  });

  it("keeps a placed finder placed while it hears nothing", () => {
    const quiet = dfState(true);
    const reading = readingOf(quiet, "df");
    if (reading === null) {
      throw new Error("a df reading");
    }
    const silent: ProcessorState = {
      ...quiet,
      reading: { type: "df", reading: { ...reading, peaks: [], squelched: true } },
    };
    const overlay = dfOverlay(
      sources({ finders: ["finder"] }),
      {},
      {},
      { finder: silent },
      NOW,
      null,
    );
    expect(overlay?.unplaced).toEqual([]);
    const headless: ProcessorState = {
      ...quiet,
      reading: { type: "df", reading: { ...reading, azimuth_deg: null } },
    };
    const lost = dfOverlay(
      sources({ finders: ["finder"] }),
      {},
      {},
      { finder: headless },
      NOW,
      null,
    );
    expect(lost?.unplaced).toEqual(["North DF"]);
  });

  it("takes the estimate, nav target and stations from where the bearings cross", () => {
    const fused: DfFusionState = {
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
    const overlay = dfOverlay(
      sources({ crossings: ["cross"] }),
      {},
      { cross: fused },
      {},
      NOW,
      HERE,
    );
    expect(overlay?.estimate?.converged).toBe(true);
    expect(overlay?.nav?.kind).toBe("estimate");
    expect(overlay?.stations).toHaveLength(1);
    expect(
      dfOverlay(sources({ crossings: ["cross"] }), {}, {}, {}, NOW, null)?.estimate,
    ).toBeNull();
  });
});
