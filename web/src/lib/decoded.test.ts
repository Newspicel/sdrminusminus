import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FLUSH_MS, RING_CAPACITY, STATION_CAPACITY, useDecodedStore } from "./decoded";
import type { AdsbMessage, DecodedRecord, RdsUpdate } from "./types";

const T0 = Date.parse("2026-08-09T12:00:00Z");

function at(offsetMs: number): string {
  return new Date(T0 + offsetMs).toISOString();
}

function adsb(data: Partial<AdsbMessage> & { icao: string }, offsetMs = 0): DecodedRecord {
  return {
    at: at(offsetMs),
    device_set: 0,
    channel: 1,
    freq_hz: 1_090_000_000,
    event: { kind: "adsb", data: { df: 17, raw: "8d4840d6", ...data } },
  };
}

function rds(data: Partial<RdsUpdate> = {}, offsetMs = 0): DecodedRecord {
  return {
    at: at(offsetMs),
    device_set: 0,
    channel: 2,
    freq_hz: 100_300_000,
    event: { kind: "rds", data: { groups: 1, block_errors: 0, pi: "D3C2", ...data } },
  };
}

function rtty(text: string, offsetMs = 0): DecodedRecord {
  return {
    at: at(offsetMs),
    device_set: 0,
    channel: 3,
    freq_hz: 14_083_000,
    event: { kind: "rtty", data: { text } },
  };
}

function push(...records: DecodedRecord[]): void {
  for (const record of records) {
    useDecodedStore.getState().push(record);
  }
  useDecodedStore.getState().flush();
}

beforeEach(() => {
  vi.useFakeTimers();
  useDecodedStore.getState().clear();
});

afterEach(() => {
  useDecodedStore.getState().clear();
  vi.useRealTimers();
});

describe("frame ring", () => {
  it("keeps the newest RING_CAPACITY frames and drops the oldest", () => {
    const overflow = 10;
    for (let i = 0; i < RING_CAPACITY + overflow; i++) {
      useDecodedStore.getState().push(adsb({ icao: "abc123", raw: `frame-${i}` }, i));
    }
    useDecodedStore.getState().flush();

    const frames = useDecodedStore.getState().frames.adsb ?? [];
    expect(frames).toHaveLength(RING_CAPACITY);
    // Newest first, mirroring GET /api/decoderlog.
    expect(frames[0]?.event.data.raw).toBe(`frame-${RING_CAPACITY + overflow - 1}`);
    expect(frames.at(-1)?.event.data.raw).toBe(`frame-${overflow}`);
    expect(useDecodedStore.getState().received).toBe(RING_CAPACITY + overflow);
  });

  it("bounds a single batch larger than the ring", () => {
    for (let i = 0; i < RING_CAPACITY * 2; i++) {
      useDecodedStore.getState().push(adsb({ icao: "abc123", raw: `frame-${i}` }, i));
    }
    useDecodedStore.getState().flush();

    const frames = useDecodedStore.getState().frames.adsb ?? [];
    expect(frames).toHaveLength(RING_CAPACITY);
    expect(frames[0]?.event.data.raw).toBe(`frame-${RING_CAPACITY * 2 - 1}`);
  });
});

describe("per-kind slices", () => {
  it("routes each frame to its own decoder's slice", () => {
    push(adsb({ icao: "abc123" }), rds(), rds({ ps: "RADIO 1" }), rtty("CQ"));

    const state = useDecodedStore.getState();
    expect(state.frames.adsb).toHaveLength(1);
    expect(state.frames.rds).toHaveLength(2);
    expect(state.frames.rtty).toHaveLength(1);
    expect(state.frames.ais).toBeUndefined();
  });

  it("leaves an untouched slice identical so its panel does not re-render", () => {
    push(rds());
    const before = useDecodedStore.getState().frames.rds;

    push(adsb({ icao: "abc123" }));

    expect(useDecodedStore.getState().frames.rds).toBe(before);
    expect(useDecodedStore.getState().frames.adsb).not.toBe(before);
  });

  it("publishes one update per flush interval, not per frame", () => {
    let updates = 0;
    const unsubscribe = useDecodedStore.subscribe(() => {
      updates++;
    });

    for (let i = 0; i < 50; i++) {
      useDecodedStore.getState().push(adsb({ icao: "abc123" }, i));
    }
    expect(useDecodedStore.getState().frames.adsb).toBeUndefined();

    vi.advanceTimersByTime(FLUSH_MS);
    unsubscribe();

    expect(updates).toBe(1);
    expect(useDecodedStore.getState().frames.adsb).toHaveLength(50);
  });
});

describe("stations", () => {
  it("merges partial ADS-B frames into one accumulating row", () => {
    push(
      adsb({ icao: "abc123", callsign: "DLH400", type_code: 4 }, 0),
      adsb({ icao: "abc123", lat: 52.5, lon: 13.4, altitude_ft: 37_000, type_code: 11 }, 1_000),
    );

    const stations = useDecodedStore.getState().stations.adsb ?? [];
    expect(stations).toHaveLength(1);
    const station = stations[0];
    expect(station?.id).toBe("abc123");
    expect(station?.frames).toBe(2);
    expect(station?.lastSeen).toBe(T0 + 1_000);
    // The position frame carries no callsign; the identity frame's must survive it.
    expect(station?.event.data).toMatchObject({
      callsign: "DLH400",
      lat: 52.5,
      lon: 13.4,
      altitude_ft: 37_000,
      type_code: 11,
    });
  });

  it("keeps one row per emitter and per decoder", () => {
    push(adsb({ icao: "abc123" }), adsb({ icao: "def456" }), rds());

    expect(useDecodedStore.getState().stations.adsb).toHaveLength(2);
    expect(useDecodedStore.getState().stations.rds).toHaveLength(1);
  });

  it("has no rows for character-stream decoders", () => {
    push(rtty("CQ CQ"));

    expect(useDecodedStore.getState().frames.rtty).toHaveLength(1);
    expect(useDecodedStore.getState().stations.rtty).toBeUndefined();
  });
});

describe("ageOut", () => {
  it("drops stations unseen for longer than the horizon", () => {
    push(adsb({ icao: "stale" }, 0), adsb({ icao: "fresh" }, 60_000));

    useDecodedStore.getState().ageOut(30_000, T0 + 60_000);

    const stations = useDecodedStore.getState().stations.adsb ?? [];
    expect(stations.map((s) => s.id)).toEqual(["fresh"]);
  });

  it("does not touch state when every station is fresh", () => {
    push(adsb({ icao: "fresh" }, 0));
    const before = useDecodedStore.getState().stations.adsb;

    useDecodedStore.getState().ageOut(30_000, T0 + 1_000);

    expect(useDecodedStore.getState().stations.adsb).toBe(before);
  });

  it("re-adds an aged-out station from scratch when it returns", () => {
    push(adsb({ icao: "abc123", callsign: "DLH400" }, 0));
    useDecodedStore.getState().ageOut(30_000, T0 + 60_000);

    push(adsb({ icao: "abc123" }, 90_000));

    const stations = useDecodedStore.getState().stations.adsb ?? [];
    expect(stations).toHaveLength(1);
    expect(stations[0]?.frames).toBe(1);
    expect(stations[0]?.event.data.callsign).toBeUndefined();
  });
});

describe("loss and clear", () => {
  it("accumulates the frames the server reported as dropped", () => {
    const { observe } = useDecodedStore.getState();
    observe({ type: "DecodedLost", data: { count: 12 } });
    observe({ type: "DecodedLost", data: { count: 3 } });

    expect(useDecodedStore.getState().lost).toBe(15);
  });

  it("routes a Decoded event into the stream", () => {
    useDecodedStore.getState().observe({ type: "Decoded", data: adsb({ icao: "abc123" }) });
    useDecodedStore.getState().flush();

    expect(useDecodedStore.getState().frames.adsb).toHaveLength(1);
  });

  it("resets frames, stations, counters and anything still staged", () => {
    push(adsb({ icao: "abc123" }), rds());
    useDecodedStore.getState().reportLost(4);
    useDecodedStore.getState().push(adsb({ icao: "def456" }));

    useDecodedStore.getState().clear();

    expect(useDecodedStore.getState()).toMatchObject({
      frames: {},
      stations: {},
      lost: 0,
      received: 0,
    });

    // The staged frame was discarded, not just hidden until the next flush.
    vi.advanceTimersByTime(FLUSH_MS);
    expect(useDecodedStore.getState().frames.adsb).toBeUndefined();
  });
});

describe("station capacity", () => {
  it("evicts the least recently seen once the cap is passed", () => {
    const store = useDecodedStore.getState();
    store.clear();
    // POCSAG has a station identity but no view that drives ageOut, so the cap is the only
    // thing standing between a busy pager frequency and unbounded growth.
    const overflow = 50;
    for (let i = 0; i < STATION_CAPACITY + overflow; i += 1) {
      store.push({
        at: at(i * 1000),
        device_set: 0,
        channel: 3,
        freq_hz: 466_230_000,
        event: {
          kind: "pocsag",
          data: {
            address: i,
            function: 3,
            baud: 1200,
            payload: "alpha",
            text: `page ${i}`,
            errors_corrected: 0,
          },
        },
      });
    }
    store.flush();
    const stations = useDecodedStore.getState().stations.pocsag ?? [];
    expect(stations.length).toBe(STATION_CAPACITY);
    // The survivors are the newest: the oldest `overflow` addresses went.
    const ids = new Set(stations.map((s) => s.id));
    expect(ids.has("0")).toBe(false);
    expect(ids.has(String(STATION_CAPACITY + overflow - 1))).toBe(true);
  });
});

describe("backlog", () => {
  it("rebuilds the station picture a reload would otherwise have lost", () => {
    useDecodedStore.getState().observe({
      type: "DecodedBacklog",
      data: {
        records: [
          adsb({ icao: "abc123", callsign: "DLH400", type_code: 4 }, 0),
          adsb({ icao: "abc123", lat: 52.5, lon: 13.4, type_code: 11 }, 1_000),
          adsb({ icao: "def456", lat: 48.1, lon: 11.6, type_code: 11 }, 2_000),
        ],
      },
    });

    const stations = useDecodedStore.getState().stations.adsb ?? [];
    expect(stations).toHaveLength(2);
    expect(stations[0]?.event.data).toMatchObject({ callsign: "DLH400", lat: 52.5, lon: 13.4 });
  });

  it("leaves the frame ring alone, because the decoder log already holds these", () => {
    useDecodedStore.getState().observe({
      type: "DecodedBacklog",
      data: { records: [adsb({ icao: "abc123" })] },
    });

    expect(useDecodedStore.getState().frames.adsb).toBeUndefined();
    expect(useDecodedStore.getState().received).toBe(0);
  });

  it("is unharmed by a live frame the backlog already carried", () => {
    const duplicate = adsb({ icao: "abc123", callsign: "DLH400", type_code: 4 }, 0);
    useDecodedStore.getState().observe({
      type: "DecodedBacklog",
      data: { records: [duplicate] },
    });
    push(duplicate);

    const stations = useDecodedStore.getState().stations.adsb ?? [];
    expect(stations).toHaveLength(1);
    expect(stations[0]?.event.data).toMatchObject({ callsign: "DLH400" });
    expect(stations[0]?.lastSeen).toBe(T0);
  });

  it("ignores records for decoders that do not accumulate into a target", () => {
    useDecodedStore.getState().observe({
      type: "DecodedBacklog",
      data: { records: [rtty("CQ CQ")] },
    });

    expect(useDecodedStore.getState().stations.rtty).toBeUndefined();
  });
});
