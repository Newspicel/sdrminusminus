import type { GeoJSONSource, Map as MapLibreMap } from "maplibre-gl";
import { describe, expect, it, vi } from "vitest";
import type { TargetCollection } from "../lib/map/layers";
import type { PositionSample } from "../lib/position";
import {
  framePositionOnce,
  frameSignalOnce,
  frameTargetsOnce,
  positionCollection,
  signalCollection,
  updatePositionSources,
  updateSignalSource,
} from "./MapPanel";

function sample(latitude: number, longitude: number, receivedAt: number): PositionSample {
  return {
    latitude,
    longitude,
    time: "2026-08-14T12:00:00Z",
    receivedAt,
  };
}

describe("MapPanel position data", () => {
  it("updates point and route sources and marks only the active track's newest fix", () => {
    const points = { setData: vi.fn() } as unknown as Pick<GeoJSONSource, "setData">;
    const route = { setData: vi.fn() } as unknown as Pick<GeoJSONSource, "setData">;
    const tracks = [
      { samples: [sample(10, 20, 1), sample(11, 21, 2)], active: false },
      { samples: [sample(30, 40, 3), sample(31, 41, 4)], active: true },
    ];

    const collection = updatePositionSources(points, route, tracks);

    expect(points.setData).toHaveBeenCalledWith(collection.points);
    expect(route.setData).toHaveBeenCalledWith(collection.route);
    expect(collection.points.features.filter((feature) => feature.properties.latest)).toEqual([
      collection.points.features[3],
    ]);
    expect(collection.route.features).toHaveLength(2);
  });

  it("publishes empty collections after position rewiring clears every track", () => {
    const points = { setData: vi.fn() } as unknown as Pick<GeoJSONSource, "setData">;
    const route = { setData: vi.fn() } as unknown as Pick<GeoJSONSource, "setData">;

    const collection = updatePositionSources(points, route, []);

    expect(collection.points.features).toEqual([]);
    expect(collection.route.features).toEqual([]);
    expect(points.setData).toHaveBeenLastCalledWith(collection.points);
    expect(route.setData).toHaveBeenLastCalledWith(collection.route);
  });
});

describe("MapPanel signal survey data", () => {
  it("publishes dBFS measurements as point properties", () => {
    const source = { setData: vi.fn() } as unknown as Pick<GeoJSONSource, "setData">;
    const samples = [
      {
        latitude: 52.52,
        longitude: 13.405,
        frequencyHz: 145_500_000,
        levelDbfs: -64.5,
        measuredAt: 1,
        observations: 2,
      },
    ];

    const collection = updateSignalSource(source, samples);

    expect(source.setData).toHaveBeenCalledWith(collection);
    expect(collection.features[0]).toMatchObject({
      geometry: { coordinates: [13.405, 52.52] },
      properties: { level: -64.5, observations: 2 },
    });
  });
});

describe("MapPanel auto framing", () => {
  it("frames the first GPS fix even when targets arrived first", () => {
    const fitBounds = vi.fn();
    const map = { fitBounds } as unknown as Pick<MapLibreMap, "fitBounds">;
    const targetFlag = { current: false };
    const positionFlag = { current: false };
    const targets: TargetCollection = {
      type: "FeatureCollection",
      features: [
        {
          type: "Feature",
          geometry: { type: "Point", coordinates: [13.4, 52.5] },
          properties: { id: "ABC123", label: "ABC123" },
        },
      ],
    };
    const positions = positionCollection([
      { samples: [sample(48.1, 11.5, 1)], active: true },
    ]).points;

    frameTargetsOnce(map, targets, targetFlag);
    framePositionOnce(map, positions, positionFlag);

    expect(fitBounds).toHaveBeenCalledTimes(2);
    expect(targetFlag.current).toBe(true);
    expect(positionFlag.current).toBe(true);
    expect(fitBounds).toHaveBeenLastCalledWith(
      [
        [11.5, 48.1],
        [11.5, 48.1],
      ],
      { padding: 56, maxZoom: 14, duration: 0 },
    );
  });

  it("frames a signal survey once without stealing a manually moved view", () => {
    const fitBounds = vi.fn();
    const map = { fitBounds } as unknown as Pick<MapLibreMap, "fitBounds">;
    const framed = { current: false };
    const signals = signalCollection([
      {
        latitude: 48.1,
        longitude: 11.5,
        frequencyHz: 145_500_000,
        levelDbfs: -70,
        measuredAt: 1,
        observations: 1,
      },
    ]);

    frameSignalOnce(map, signals, framed);
    frameSignalOnce(map, signals, framed);

    expect(fitBounds).toHaveBeenCalledTimes(1);
    expect(framed.current).toBe(true);
  });
});
