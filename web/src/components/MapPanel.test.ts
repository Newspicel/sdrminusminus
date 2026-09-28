import type { GeoJSONSource, Map as MapLibreMap } from "maplibre-gl";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import { clientEvents, resetEvents } from "../lib/diagnostics";
import type { TargetCollection } from "../lib/map/layers";
import {
  framePositionOnce,
  frameSignalOnce,
  frameTargetsOnce,
  positionCollection,
  setSourceData,
  signalCollection,
  updatePositionSources,
  updateSignalSource,
} from "../lib/map/sources";
import { installTargetLayers } from "../lib/map/targets";
import type { PositionSample } from "../lib/position";
import { MapLegend } from "./map/MapLegend";
import { ZERO_COUNTS } from "./map/mapState";

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
        frequency_hz: 145_500_000,
        level_dbfs: -64.5,
        measured_at: "2026-09-29T12:00:00Z",
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
        frequency_hz: 145_500_000,
        level_dbfs: -70,
        measured_at: "2026-09-29T12:00:00Z",
        observations: 1,
      },
    ]);

    frameSignalOnce(map, signals, framed);
    frameSignalOnce(map, signals, framed);

    expect(fitBounds).toHaveBeenCalledTimes(1);
    expect(framed.current).toBe(true);
  });
});

describe("MapPanel failures", () => {
  afterEach(() => {
    resetEvents();
    vi.unstubAllGlobals();
  });

  it("reports map data the map refuses", async () => {
    const source = { setData: vi.fn(() => Promise.reject(new Error("bad geometry"))) };
    setSourceData(source, { type: "FeatureCollection", features: [] });
    await vi.waitFor(() =>
      expect(clientEvents().some((event) => event.message.includes("bad geometry"))).toBe(true),
    );
  });

  it("says when heading icons cannot be drawn", () => {
    vi.stubGlobal("document", {
      createElement: () => ({ width: 0, height: 0, getContext: () => null }),
    });
    const map = {
      getLayer: () => undefined,
      getSource: () => undefined,
      hasImage: () => false,
      addImage: vi.fn(),
      addSource: vi.fn(),
      addLayer: vi.fn(),
    };
    expect(installTargetLayers(map as unknown as MapLibreMap, "#000000", ["adsb"])).toBe(false);
    expect(clientEvents().some((event) => event.message === "heading icons failed")).toBe(true);
    expect(map.addLayer).toHaveBeenCalledWith(expect.objectContaining({ id: "targets-adsb-dot" }));
  });
});

describe("MapLegend", () => {
  const quiet = { bearings: 0, echoes: 0, tracks: 0, unplaced: [] };

  function legend(overrides: Partial<Parameters<typeof MapLegend>[0]> = {}): string {
    return renderToStaticMarkup(
      createElement(MapLegend, {
        kinds: [],
        counts: ZERO_COUNTS,
        positionCount: null,
        signalCells: null,
        overlay: quiet,
        heat: false,
        headings: true,
        basemap: "online",
        ...overrides,
      }),
    );
  }

  it("shows overlay rows only when they hold something", () => {
    expect(legend()).toBe(
      '<div class="pointer-events-none absolute top-2 left-2 flex flex-col items-start gap-1"></div>',
    );
    const html = legend({
      overlay: { bearings: 3, echoes: 2, tracks: 1, unplaced: ["North DF", "Radar"] },
      heat: true,
    });
    for (const row of ["Bearings", "Echoes", "Tracks", "Heat", "No position"]) {
      expect(html).toContain(row);
    }
    expect(html).toMatch(/pointer-events-auto" title="North DF, Radar"/);
  });

  it("badges a blank basemap and missing headings", () => {
    const html = legend({ basemap: "blank", headings: false });
    expect(html).toContain("no basemap");
    expect(html).toContain("no headings");
  });
});
