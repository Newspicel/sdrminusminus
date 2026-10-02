import { describe, expect, it } from "vitest";
import type {
  AdsbMessage,
  AisMessage,
  AprsPacket,
  ChannelParams,
  DecodedRecordOf,
  RadiosondeFrame,
} from "../types";
import {
  isStale,
  layerId,
  MAP_KINDS,
  mapKindsOf,
  radiosondeTracks,
  referenceCollection,
  referencePositions,
  sourceId,
  TARGET_MAX_AGE_MS,
  type Target,
  TRACK_KINDS,
  targetCollection,
  targetDetail,
  targetFeature,
  targetHeading,
  targetLabel,
  trackSourceId,
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

function sonde(data: Partial<RadiosondeFrame>, over: Partial<Target> = {}): Target {
  return station(
    {
      kind: "radiosonde",
      data: { sonde: "rs41", serial: "S1234567", errors_corrected: 0, ...data },
    },
    over,
  );
}

function sondeRecord(
  serial: string,
  offsetMs: number,
  lat: number | null,
  lon: number | null,
): DecodedRecordOf<"radiosonde"> {
  return {
    at: new Date(NOW + offsetMs).toISOString(),
    device_set: 0,
    channel: 0,
    freq_hz: 403_000_000,
    event: { kind: "radiosonde", data: { sonde: "rs41", serial, errors_corrected: 0, lat, lon } },
  };
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

  it("gives track kinds their own source and layer", () => {
    const ids = [
      ...MAP_KINDS.map(sourceId),
      ...TRACK_KINDS.flatMap((kind) => [trackSourceId(kind), layerId(kind, "track")]),
    ];
    expect(new Set(ids).size).toBe(ids.length);
  });
});

describe("radiosondeTracks", () => {
  it("draws each sonde's positions oldest first", () => {
    const records = [
      sondeRecord("A", 2_000, 48.2, 11.2),
      sondeRecord("B", 1_500, 50, 8),
      sondeRecord("A", 1_000, null, null),
      sondeRecord("A", 0, 48.1, 11.1),
    ];
    expect(radiosondeTracks(records, NOW + 2_000)).toEqual({
      type: "FeatureCollection",
      features: [
        {
          type: "Feature",
          geometry: {
            type: "LineString",
            coordinates: [
              [11.1, 48.1],
              [11.2, 48.2],
            ],
          },
          properties: { id: "A" },
        },
      ],
    });
  });

  it("drops a sonde not heard within the horizon", () => {
    const records = [sondeRecord("A", 1_000, 48.2, 11.2), sondeRecord("A", 0, 48.1, 11.1)];
    expect(radiosondeTracks(records, NOW + TARGET_MAX_AGE_MS + 2_000).features).toEqual([]);
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
    expect(targetLabel(sonde({}))).toBe("S1234567");
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

  it("reads a sonde's heading", () => {
    expect(targetHeading(sonde({ heading_deg: 45 }))).toBe(45);
    expect(targetHeading(sonde({}))).toBeNull();
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

  it("shows a sonde's flight data with units", () => {
    const detail = targetDetail(
      sonde({ altitude_m: 15_234.4, climb_ms: 5.12, temperature_c: -60.25, pressure_hpa: 120 }),
    );
    expect(detail.rows).toEqual([
      ["Serial", "S1234567"],
      ["Altitude", "15234 m"],
      ["Climb", "5.1 m/s"],
      ["Temp", "-60.3 °C"],
      ["Pressure", "120.0 hPa"],
      ["Frames", "1"],
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
