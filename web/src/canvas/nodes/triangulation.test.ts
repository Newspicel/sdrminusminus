import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it } from "vitest";
import { useFusionStore } from "../../lib/fusion";
import type { DfEstimate, DfFusionState, DfStation, PatchGraph, PatchNode } from "../../lib/types";
import { CATALOG, catalogBody } from "../../test/catalog";
import { renderFace } from "../../test/faceHarness";
import { placed } from "../../test/fixtures";
import { HeatMarks } from "./FusionHeat";
import { TriangulationFace } from "./TriangulationFace";
import {
  decayWith,
  estimateLabel,
  fusionSources,
  geoToGrid,
  guidanceText,
  insideBox,
  NO_SOURCES,
  spreadLabel,
  stationAge,
  stationBearing,
  stationSigma,
  triangulationSettings,
} from "./triangulation";

function station(last_seen: string, over: Partial<DfStation> = {}): DfStation {
  return { station_id: "north", lat: 51.5, lon: 7.0, bearings: 3, last_seen, ...over };
}

const TRI: PatchNode = placed("tri", catalogBody("triangulation"));

function wire(from: string, to: string, port = "events") {
  return { from: { node: from, port: "events" }, to: { node: to, port } };
}

const WIRED: PatchGraph = {
  nodes: [
    placed("finder", catalogBody("df")),
    placed("walk", catalogBody("hunt")),
    placed("filter", { kind: "event_filter" }),
    placed("log", { kind: "decoder_log" }),
    TRI,
  ],
  edges: [
    wire("finder", "tri"),
    wire("finder", "tri"),
    wire("walk", "tri"),
    wire("filter", "tri"),
    wire("log", "tri"),
    wire("finder", "log"),
  ],
};

const FUSED: DfFusionState = {
  estimate: {
    lat: 51.95012,
    lon: 13.05033,
    ellipse_major_m: 120,
    ellipse_minor_m: 40,
    ellipse_bearing_deg: 30,
    converged: true,
    samples: 7,
  },
  nav: {
    lat: 51.95,
    lon: 13.05,
    kind: "probe",
    revision: 2,
    distance_m: 2_400,
    bearing_deg: 123.4,
  },
  stations: [
    station("2026-01-01T00:09:58Z", {
      station_id: "roof",
      last_bearing_deg: 137.04,
      sigma_deg: 3.1,
    }),
  ],
  samples: 7,
  dropped: 2,
  refused: 0,
};

afterEach(() => {
  useFusionStore.setState({ byNode: {} });
});

describe("spreadLabel", () => {
  it("reads the ellipse in units a driver can judge", () => {
    expect(
      spreadLabel({
        lat: 51.5,
        lon: 7.0,
        ellipse_major_m: 2_400,
        ellipse_minor_m: 180,
        ellipse_bearing_deg: 45,
        converged: false,
        samples: 6,
      }),
    ).toBe("2.4 km × 180 m");
    expect(spreadLabel(null)).toBe("-");
    expect(estimateLabel(null)).toBe("-");
    expect(estimateLabel(FUSED.estimate ?? null)).toBe("51.95012, 13.05033");
  });
});

describe("stationAge", () => {
  const now = Date.parse("2026-01-01T00:10:00Z");

  it("says how long ago a finder last reported", () => {
    expect(stationAge(station("2026-01-01T00:09:58Z"), now)).toBe("just now");
    expect(stationAge(station("2026-01-01T00:09:30Z"), now)).toBe("30s ago");
    expect(stationAge(station("2026-01-01T00:05:00Z"), now)).toBe("5m ago");
  });

  it("does not pretend to know an unreadable time", () => {
    expect(stationAge(station("who knows"), now)).toBe("just now");
  });

  it("shows the last bearing and its sigma, or a dash", () => {
    expect(stationBearing(station("", { last_bearing_deg: 137.04 }))).toBe("137.0°");
    expect(stationBearing(station(""))).toBe("-");
    expect(stationSigma(station("", { sigma_deg: 3.14 }))).toBe("3.1°");
    expect(stationSigma(station("", { sigma_deg: 0 }))).toBe("-");
  });
});

describe("geoToGrid", () => {
  const frame = { south: 51.9, west: 12.9, north: 52.1, east: 13.3 };
  const box = { w: 400, h: 180 };

  it("maps corners of the grid to corners of the box", () => {
    expect(geoToGrid({ lat: 52.1, lon: 12.9 }, frame, box)).toEqual({ x: 0, y: 0 });
    expect(geoToGrid({ lat: 51.9, lon: 13.3 }, frame, box)).toEqual({ x: 400, y: 180 });
    const middle = geoToGrid({ lat: 52.0, lon: 13.1 }, frame, box);
    expect(middle.x).toBeCloseTo(200, 6);
    expect(middle.y).toBeCloseTo(90, 6);
  });

  it("keeps a point outside the grid off the box and survives an empty grid", () => {
    expect(insideBox(geoToGrid({ lat: 52.2, lon: 13.1 }, frame, box), box)).toBe(false);
    const flat = { south: 52, west: 13, north: 52, east: 13 };
    expect(geoToGrid({ lat: 52, lon: 13 }, flat, box)).toEqual({ x: 200, y: 90 });
  });
});

describe("HeatMarks", () => {
  it("draws stations, the estimate and other emitters where the grid puts them", () => {
    const estimate: DfEstimate = {
      lat: 1,
      lon: 2,
      ellipse_major_m: 120,
      ellipse_minor_m: 40,
      ellipse_bearing_deg: 30,
      converged: true,
      samples: 7,
    };
    const html = renderToStaticMarkup(
      createElement(HeatMarks, {
        bounds: { south: 0, west: 0, north: 2, east: 4 },
        box: { w: 400, h: 200 },
        estimate,
        emitters: [estimate, { ...estimate, lat: 0.5, lon: 1 }],
        stations: [
          station("", { station_id: "roof", lat: 2, lon: 0 }),
          station("", { station_id: "far", lat: 10, lon: 2 }),
        ],
      }),
    );
    expect(html).toMatch(/<circle cx="0" cy="0"[^>]*><title>roof<\/title>/);
    expect(html).not.toContain("far");
    expect(html).toContain('d="M194 100H206M200 94V106" class="stroke-accent stroke-2"');
    expect(html).toContain('d="M94 150H106M100 144V156" class="stroke-accent/50"');
    expect(html.match(/<path/g)).toHaveLength(2);
  });
});

describe("fusionSources", () => {
  it("counts each wired finder, hunt and filter once", () => {
    expect(fusionSources(WIRED, "tri")).toBe(3);
    expect(fusionSources(WIRED, "log")).toBe(1);
    expect(fusionSources({ nodes: [TRI], edges: [] }, "tri")).toBe(0);
  });
});

describe("guidanceText", () => {
  it("says which way to drive and how far", () => {
    expect(guidanceText(FUSED)).toEqual({ text: "Drive across · 123°", title: "2.4 km away" });
  });

  it("says why there is no guidance", () => {
    expect(guidanceText(undefined)).toEqual({ text: "-" });
    expect(guidanceText({ samples: 0, no_guide_position: true }).title).toBe(
      "Wire a GPS in to guide it",
    );
    expect(guidanceText({ samples: 0, no_bearings: true }).title).toBe(
      "No bearings to steer by yet",
    );
  });
});

describe("decay", () => {
  it("keeps a chosen half life and starts a new one at five minutes", () => {
    expect(decayWith("fixed", { kind: "auto" })).toEqual({ kind: "fixed" });
    expect(decayWith("half_life", { kind: "auto" })).toEqual({
      kind: "half_life",
      seconds: 300,
    });
    expect(decayWith("half_life", { kind: "half_life", seconds: 42 })).toEqual({
      kind: "half_life",
      seconds: 42,
    });
  });

  it("falls back to the catalog settings", () => {
    expect(triangulationSettings(TRI, CATALOG)?.decay).toEqual({ kind: "auto" });
    expect(triangulationSettings(placed("tri", { kind: "triangulation" }), CATALOG)).toEqual(
      triangulationSettings(TRI, CATALOG),
    );
    expect(triangulationSettings(placed("walk", catalogBody("hunt")), CATALOG)).toBeNull();
  });
});

describe("TriangulationFace", () => {
  it("asks for finders when nothing is wired", () => {
    const html = renderFace(TriangulationFace, TRI, { graph: { nodes: [TRI], edges: [] } });
    expect(html).toContain("0 of 0 reporting");
    expect(html).toContain(NO_SOURCES);
    expect(html).toContain(">Clear<");
    expect(html).toContain("Fade");
  });

  it("shows the estimate, guidance, stations and lost bearings", () => {
    useFusionStore.setState({ byNode: { tri: FUSED } });
    const html = renderFace(TriangulationFace, TRI, { graph: WIRED });
    expect(html).toContain("1 of 3 reporting");
    expect(html).toContain("51.95012, 13.05033");
    expect(html).toContain("120 m × 40");
    expect(html).toContain("Drive across · 123°");
    expect(html).toContain("roof");
    expect(html).toContain("137.0°");
    expect(html).toContain("3.1°");
    expect(html).toContain("Dropped");
    expect(html).not.toContain("Refused");
    expect(html).not.toContain(NO_SOURCES);
  });
});
