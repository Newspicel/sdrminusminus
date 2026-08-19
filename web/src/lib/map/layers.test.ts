import { describe, expect, it } from "vitest";
import type { AdsbMessage, AisMessage, AprsPacket, ChannelParams } from "../types";
import {
  isStale,
  layerId,
  MAP_KINDS,
  mapKindsOf,
  referenceCollection,
  referencePositions,
  sourceId,
  TARGET_MAX_AGE_MS,
  type Target,
  targetCollection,
  targetDetail,
  targetFeature,
  targetHeading,
  targetLabel,
} from "./layers";

const NOW = Date.parse("2026-08-09T12:00:00Z");

function adsb(data: Partial<AdsbMessage>, over: Partial<Target> = {}): Target {
  return station({ kind: "adsb", data: { df: 17, icao: "3c6444", raw: "8d", ...data } }, over);
}

function ais(data: Partial<AisMessage>, over: Partial<Target> = {}): Target {
  return station(
    {
      kind: "ais",
      data: { ais_channel: "A", mmsi: 211234560, msg_type: 1, nmea: "!AIVDM", ...data },
    },
    over,
  );
}

function aprs(data: Partial<AprsPacket>, over: Partial<Target> = {}): Target {
  return station(
    {
      kind: "aprs",
      data: {
        destination: "APRS",
        info: "!",
        source: "DL1ABC-9",
        tnc2: "DL1ABC-9>APRS:!",
        ...data,
      },
    },
    over,
  );
}

function station(event: Target["event"], over: Partial<Target>): Target {
  return {
    kind: event.kind,
    id: "target",
    event,
    lastSeen: NOW,
    freqHz: 1_090_000_000,
    deviceSet: 0,
    channel: 0,
    frames: 1,
    ...over,
  };
}

describe("MAP_KINDS", () => {
  it("names a source and three layers per kind, all distinct", () => {
    const ids = MAP_KINDS.flatMap((kind) => [
      sourceId(kind),
      layerId(kind, "dot"),
      layerId(kind, "heading"),
      layerId(kind, "label"),
    ]);
    expect(new Set(ids).size).toBe(ids.length);
  });
});

describe("mapKindsOf", () => {
  it("keeps only the decoders that report a position", () => {
    expect(mapKindsOf(["adsb", "pocsag", "acars"])).toEqual(["adsb"]);
    expect(mapKindsOf(["pocsag", "rtty"])).toEqual([]);
    expect(mapKindsOf([])).toEqual([]);
  });

  it("deduplicates and orders by MAP_KINDS, not by wire order", () => {
    expect(mapKindsOf(["aprs", "adsb", "aprs"])).toEqual(["adsb", "aprs"]);
    expect(mapKindsOf(["ais", "adsb"])).toEqual(mapKindsOf(["adsb", "ais"]));
  });
});

describe("targetFeature", () => {
  it("emits GeoJSON [lon, lat] order with a label and heading", () => {
    const feature = targetFeature(
      adsb({ lat: 52.5163, lon: 13.3777, callsign: "DLH123 ", track_deg: 271.5 }),
    );
    expect(feature).toEqual({
      type: "Feature",
      geometry: { type: "Point", coordinates: [13.3777, 52.5163] },
      properties: { id: "target", label: "DLH123", heading: 271.5 },
    });
  });

  it("omits heading rather than defaulting it to north", () => {
    const feature = targetFeature(adsb({ lat: 1, lon: 2 }));
    expect(feature?.properties).not.toHaveProperty("heading");
  });

  it("drops a target with no position yet", () => {
    expect(targetFeature(adsb({ callsign: "DLH123" }))).toBeNull();
    expect(targetFeature(adsb({ lat: 52.5 }))).toBeNull();
  });

  it("drops out-of-range sentinel positions", () => {
    expect(targetFeature(ais({ lat: 91, lon: 181 }))).toBeNull();
    expect(targetFeature(ais({ lat: 0, lon: 0 }))).not.toBeNull();
  });
});

describe("targetLabel", () => {
  it("falls back through each kind's identities", () => {
    expect(targetLabel(adsb({ callsign: "  " }))).toBe("3C6444");
    expect(targetLabel(ais({}))).toBe("211234560");
    expect(targetLabel(ais({ call_sign: "DEAB" }))).toBe("DEAB");
    expect(targetLabel(ais({ name: "NORDIC", call_sign: "DEAB" }))).toBe("NORDIC");
    expect(targetLabel(aprs({}))).toBe("DL1ABC-9");
  });
});

describe("targetHeading", () => {
  it("prefers true heading over course for a vessel", () => {
    expect(targetHeading(ais({ heading_deg: 90, cog_deg: 275 }))).toBe(90);
    expect(targetHeading(ais({ cog_deg: 275 }))).toBe(275);
  });

  it("treats the not-available sentinels as no heading", () => {
    expect(targetHeading(ais({ heading_deg: 511, cog_deg: 360 }))).toBeNull();
    expect(targetHeading(ais({ heading_deg: 511, cog_deg: 12 }))).toBe(12);
  });

  it("wraps into [0, 360)", () => {
    expect(targetHeading(aprs({ course_deg: 360 }))).toBe(0);
    expect(targetHeading(aprs({ course_deg: -90 }))).toBe(270);
    expect(targetHeading(aprs({ course_deg: 450 }))).toBe(90);
    expect(targetHeading(aprs({ course_deg: Number.NaN }))).toBeNull();
  });
});

describe("isStale", () => {
  it("expires strictly older than the horizon", () => {
    expect(isStale(NOW - TARGET_MAX_AGE_MS + 1, NOW)).toBe(false);
    expect(isStale(NOW - TARGET_MAX_AGE_MS - 1, NOW)).toBe(true);
    expect(isStale(NOW - 2_000, NOW, 1_000)).toBe(true);
  });
});

describe("targetCollection", () => {
  it("keeps only positioned, fresh targets", () => {
    const stations = [
      adsb({ lat: 1, lon: 1 }, { id: "fresh" }),
      adsb({ lat: 2, lon: 2 }, { id: "stale", lastSeen: NOW - TARGET_MAX_AGE_MS - 1 }),
      adsb({}, { id: "no-fix" }),
    ];
    const collection = targetCollection(stations, NOW);
    expect(collection.type).toBe("FeatureCollection");
    expect(collection.features.map((f) => f.properties.id)).toEqual(["fresh"]);
  });

  it("is empty, not absent, with nothing to draw", () => {
    expect(targetCollection([], NOW)).toEqual({ type: "FeatureCollection", features: [] });
  });
});

const ref = (ref_lat?: number | null, ref_lon?: number | null): ChannelParams => ({
  type: "adsb",
  settings: { ref_lat, ref_lon },
});

describe("referencePositions", () => {
  it("reads only ADS-B references, in [lon, lat] order", () => {
    expect(referencePositions([ref(50.7, 6.1), { type: "nfm", settings: {} }])).toEqual([
      [6.1, 50.7],
    ]);
  });

  it("merges channels sharing one antenna into one mark", () => {
    expect(referencePositions([ref(50.7, 6.1), ref(50.7, 6.1), ref(-33.9, 18.4)])).toEqual([
      [6.1, 50.7],
      [18.4, -33.9],
    ]);
  });

  it("skips a half-set or out-of-range reference", () => {
    expect(referencePositions([ref(50.7, null), ref(50.7), ref(91, 181)])).toEqual([]);
  });
});

describe("referenceCollection", () => {
  it("wraps fixes as identity-less points", () => {
    expect(referenceCollection([[6.1, 50.7]])).toEqual({
      type: "FeatureCollection",
      features: [
        { type: "Feature", geometry: { type: "Point", coordinates: [6.1, 50.7] }, properties: {} },
      ],
    });
  });
});

describe("targetDetail", () => {
  it("lists only the fields the target actually reported", () => {
    const detail = targetDetail(
      adsb({ lat: -33.8688, lon: 151.2093, altitude_ft: 37_000, track_deg: 89.4 }, { frames: 12 }),
    );
    expect(detail.rows).toEqual([
      ["ICAO", "3C6444"],
      ["Position", "33.8688° S 151.2093° E"],
      ["Altitude", "37000 ft"],
      ["Track", "89°"],
      ["Frames", "12"],
    ]);
  });

  it("carries the identity and provenance the panel header shows", () => {
    const detail = targetDetail(ais({ name: "NORDIC" }, { id: "211234560" }));
    expect(detail).toMatchObject({
      kind: "ais",
      id: "211234560",
      label: "NORDIC",
      freqHz: 1_090_000_000,
      lastSeen: NOW,
    });
  });
});
