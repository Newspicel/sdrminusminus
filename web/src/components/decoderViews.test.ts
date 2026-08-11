import { describe, expect, it } from "vitest";
import type { StationOf } from "../lib/decoded";
import type { DecodedRecordOf, DecoderEventOf, DecoderKind, RdsUpdate } from "../lib/types";
import {
  acarsHeadline,
  acarsTag,
  ageClass,
  aircraftRow,
  appendTranscript,
  aprsMotion,
  buildTranscript,
  dvKind,
  dvMode,
  dvNetwork,
  dvParties,
  formatAge,
  formatAltFreqs,
  formatAltitudeFt,
  formatBearing,
  formatClock,
  formatPosition,
  formatRic,
  formatSpeedKt,
  functionLabel,
  inScope,
  isAtBottom,
  latestWpm,
  matchesAddress,
  navtexHeader,
  navtexQuality,
  ptyLabel,
  rdsPicture,
  rdsQuality,
  recordsInScope,
  shipRow,
  sortTargets,
  stationsInScope,
  subghzPayload,
  subghzReadings,
  subghzTiming,
  TARGET_MAX_AGE_MS,
  TARGET_STALE_MS,
} from "./decoderViews";

const NOW = Date.parse("2026-08-09T12:00:00Z");

function record<K extends DecoderKind>(
  kind: K,
  data: DecoderEventOf<K>["data"],
  over: Partial<Omit<DecodedRecordOf<K>, "event">> = {},
): DecodedRecordOf<K> {
  return {
    at: "2026-08-09T12:00:00Z",
    device_set: 1,
    channel: 0,
    freq_hz: 1_090_000_000,
    event: { kind, data } as DecoderEventOf<K>,
    ...over,
  };
}

function station<K extends DecoderKind>(
  kind: K,
  id: string,
  data: DecoderEventOf<K>["data"],
  over: Partial<StationOf<K>> = {},
): StationOf<K> {
  return {
    kind,
    id,
    event: { kind, data } as DecoderEventOf<K>,
    lastSeen: NOW,
    freqHz: 1_090_000_000,
    deviceSet: 1,
    channel: 0,
    frames: 1,
    ...over,
  };
}

describe("scoping", () => {
  it("matches everything when the scope is empty", () => {
    expect(inScope(3, 7, {})).toBe(true);
    expect(inScope(3, 7, { deviceSet: 3 })).toBe(true);
    expect(inScope(3, 7, { deviceSet: 3, channel: 7 })).toBe(true);
    expect(inScope(3, 7, { deviceSet: 2 })).toBe(false);
    expect(inScope(3, 7, { deviceSet: 3, channel: 8 })).toBe(false);
  });

  it("returns the same array identity when nothing can be filtered out", () => {
    const records = [record("rtty", { text: "a" })];
    expect(recordsInScope(records, {})).toBe(records);
    expect(recordsInScope(records, { channel: 1 })).toHaveLength(0);

    const stations = [station("adsb", "abc123", { icao: "abc123", df: 17, raw: "8d" })];
    expect(stationsInScope(stations, {})).toBe(stations);
    expect(stationsInScope(stations, { deviceSet: 9 })).toHaveLength(0);
  });
});

describe("ageing", () => {
  it("dims a target before it disappears", () => {
    expect(ageClass(0)).toBe("text-ink");
    expect(ageClass(TARGET_STALE_MS - 1)).toBe("text-ink");
    expect(ageClass(TARGET_STALE_MS)).toBe("text-ink-dim");
    expect(ageClass(TARGET_MAX_AGE_MS / 2)).toBe("text-ink-dim opacity-50");
  });

  it("formats age as seconds, m:ss then hours", () => {
    expect(formatAge(0)).toBe("0s");
    expect(formatAge(59_400)).toBe("59s");
    expect(formatAge(60_000)).toBe("1:00");
    expect(formatAge(3_599_000)).toBe("59:59");
    expect(formatAge(3_600_000)).toBe("1h00");
    expect(formatAge(-5_000)).toBe("0s");
  });
});

describe("target rows", () => {
  const aircraft = station(
    "adsb",
    "3c6444",
    {
      icao: "3c6444",
      df: 17,
      raw: "8d3c6444",
      callsign: " DLH123 ",
      altitude_ft: 37_000,
      ground_speed_kt: 451.4,
      track_deg: 271.6,
      lat: 52.52,
      lon: 13.405,
    },
    { frames: 42, lastSeen: NOW - 12_000 },
  );

  it("projects an aircraft", () => {
    expect(aircraftRow(aircraft, NOW)).toEqual({
      id: "3C6444",
      label: "DLH123",
      primary: "37,000 ft",
      secondary: "451 kt · 272°",
      position: "52.52000, 13.40500",
      ageMs: 12_000,
      frames: 42,
    });
  });

  it("shows GND instead of an altitude and never a negative age", () => {
    const onGround = station(
      "adsb",
      "3c6444",
      { icao: "3c6444", df: 17, raw: "8d", on_ground: true, altitude_ft: 0 },
      { lastSeen: NOW + 500 },
    );
    const row = aircraftRow(onGround, NOW);
    expect(row.primary).toBe("GND");
    expect(row.label).toBe("—");
    expect(row.position).toBe("—");
    expect(row.ageMs).toBe(0);
  });

  it("falls back from ship name to call sign", () => {
    const ship = station(
      "ais",
      "211234560",
      {
        mmsi: 211234560,
        msg_type: 1,
        ais_channel: "A",
        nmea: "!AIVDM",
        call_sign: "DEAB",
        sog_kt: 12.4,
        cog_deg: 359.7,
        destination: "HAMBURG",
      },
      { lastSeen: NOW - 90_000 },
    );
    expect(shipRow(ship, NOW)).toMatchObject({
      id: "211234560",
      label: "DEAB",
      primary: "12 kt",
      secondary: "0° · HAMBURG",
      position: "—",
      ageMs: 90_000,
    });
  });
});

describe("sortTargets", () => {
  const rows = [
    { id: "3C6444", ageMs: 5_000 },
    { id: "0A0001", ageMs: 40_000 },
    { id: "FFFFFF", ageMs: 1_000 },
  ].map((r) => ({ ...r, label: "", primary: "", secondary: "", position: "", frames: 1 }));

  it("sorts freshest first by age, and reverses on demand", () => {
    expect(sortTargets(rows, "age", false).map((r) => r.id)).toEqual([
      "FFFFFF",
      "3C6444",
      "0A0001",
    ]);
    expect(sortTargets(rows, "age", true).map((r) => r.id)).toEqual(["0A0001", "3C6444", "FFFFFF"]);
  });

  it("sorts identities by length then lexically, so MMSIs order numerically", () => {
    expect(sortTargets(rows, "id", false).map((r) => r.id)).toEqual(["0A0001", "3C6444", "FFFFFF"]);
    const mmsis = [{ id: "9" }, { id: "211234560" }, { id: "100" }].map((r) => ({
      ...r,
      label: "",
      primary: "",
      secondary: "",
      position: "",
      ageMs: 0,
      frames: 1,
    }));
    expect(sortTargets(mmsis, "id", false).map((r) => r.id)).toEqual(["9", "100", "211234560"]);
  });

  it("does not mutate its input", () => {
    const before = rows.map((r) => r.id);
    sortTargets(rows, "id", true);
    expect(rows.map((r) => r.id)).toEqual(before);
  });
});

describe("formatting", () => {
  it("groups altitudes and rounds speeds and bearings", () => {
    expect(formatAltitudeFt(null)).toBe("—");
    expect(formatAltitudeFt(900)).toBe("900 ft");
    expect(formatAltitudeFt(37_000)).toBe("37,000 ft");
    expect(formatAltitudeFt(-1_200)).toBe("−1,200 ft");
    expect(formatSpeedKt(undefined)).toBe("—");
    expect(formatSpeedKt(12.6)).toBe("13 kt");
    expect(formatBearing(null)).toBe("");
    expect(formatBearing(359.7)).toBe("0°");
    expect(formatBearing(-90)).toBe("270°");
  });

  it("needs both halves of a position", () => {
    expect(formatPosition(52.52, null)).toBe("—");
    expect(formatPosition(52.52, 13.405)).toBe("52.52000, 13.40500");
  });

  it("renders a clock, and a placeholder for an unparsable stamp", () => {
    const local = new Date(2026, 7, 9, 12, 34, 56).toISOString();
    expect(formatClock(local)).toBe("12:34:56");
    expect(formatClock("not a date")).toBe("--:--:--");
  });
});

describe("RDS", () => {
  const base: RdsUpdate = { groups: 0, block_errors: 0 };

  it("folds frames forward without a later frame erasing an earlier field", () => {
    // Newest first, as the store publishes them.
    const records = [
      record("rds", { ...base, groups: 100, block_errors: 1, radiotext: "Now playing" }),
      record("rds", { ...base, groups: 50, block_errors: 0, ps: "RADIO 1", pi: "D389" }),
    ];
    expect(rdsPicture(records)).toEqual({
      groups: 100,
      block_errors: 1,
      ps: "RADIO 1",
      pi: "D389",
      radiotext: "Now playing",
    });
    expect(rdsPicture([])).toBeNull();
  });

  it("grades block errors against the four blocks in every accepted group", () => {
    expect(rdsQuality(base).label).toBe("no lock");
    expect(rdsQuality({ ...base, groups: 1000, block_errors: 10 }).label).toBe("good");
    expect(rdsQuality({ ...base, groups: 1000, block_errors: 200 }).label).toBe("fair");
    expect(rdsQuality({ ...base, groups: 100, block_errors: 200 }).label).toBe("poor");
    expect(rdsQuality({ ...base, groups: 1000, block_errors: 0 }).errorRate).toBe(0);
  });

  it("prefers the wire's PTY name and falls back to the code", () => {
    expect(ptyLabel({ ...base, pty: 10, pty_name: "Pop Music" })).toBe("Pop Music");
    expect(ptyLabel({ ...base, pty: 10 })).toBe("PTY 10");
    expect(ptyLabel(base)).toBe("—");
  });

  it("sorts alternative frequencies ascending", () => {
    expect(formatAltFreqs([100_300_000, 98_500_000])).toEqual(["98.5 MHz", "100.3 MHz"]);
    expect(formatAltFreqs(undefined)).toEqual([]);
  });
});

describe("transcripts", () => {
  it("appends until the limit, then drops the head at a line boundary", () => {
    expect(appendTranscript("ab", "cd", 10)).toBe("abcd");
    expect(appendTranscript("abcdef", "gh", 4)).toBe("efgh");
    expect(appendTranscript("one\ntwo\nthree\n", "four\n", 12)).toBe("three\nfour\n");
  });

  it("builds the pane oldest-first from newest-first records", () => {
    const records = [record("rtty", { text: "CQ " }), record("rtty", { text: "TEST " })];
    expect(buildTranscript(records)).toBe("TEST CQ ");
    expect(buildTranscript([])).toBe("");
  });

  it("reports the most recent Morse speed only", () => {
    expect(latestWpm([])).toBeNull();
    expect(
      latestWpm([record("morse", { text: "E", wpm: 22 }), record("morse", { text: "T", wpm: 18 })]),
    ).toBe(22);
  });

  it("treats a near-bottom scroll as bottom", () => {
    expect(isAtBottom({ scrollTop: 900, scrollHeight: 1000, clientHeight: 100 })).toBe(true);
    expect(isAtBottom({ scrollTop: 895, scrollHeight: 1000, clientHeight: 100 })).toBe(true);
    expect(isAtBottom({ scrollTop: 500, scrollHeight: 1000, clientHeight: 100 })).toBe(false);
  });
});

describe("POCSAG", () => {
  it("pads RICs and labels the function bits A–D", () => {
    expect(formatRic(1234)).toBe("0001234");
    expect(functionLabel(0)).toBe("A");
    expect(functionLabel(3)).toBe("D");
    expect(functionLabel(7)).toBe("7");
  });

  it("filters on the padded RIC and ignores non-digits", () => {
    expect(matchesAddress(1234, "")).toBe(true);
    expect(matchesAddress(1234, "  ")).toBe(true);
    expect(matchesAddress(1234, "1234")).toBe(true);
    expect(matchesAddress(1234, "0001234")).toBe(true);
    expect(matchesAddress(1234, "1235")).toBe(false);
  });
});

describe("aprsMotion", () => {
  it("joins only the fields the packet carried", () => {
    expect(aprsMotion({})).toBe("");
    expect(aprsMotion({ speed_kt: 31.5 })).toBe("32 kt");
    expect(aprsMotion({ course_deg: 90, speed_kt: 10, altitude_ft: 1_500 })).toBe(
      "90° · 10 kt · 1,500 ft",
    );
  });
});

describe("NAVTEX", () => {
  it("shows the B1B2B3B4 group only when the whole header arrived", () => {
    expect(navtexHeader({ station: "D", subject: "A", serial: 7 })).toBe("DA07");
    expect(navtexHeader({ station: "D", subject: "A", serial: 12 })).toBe("DA12");
    expect(navtexHeader({ station: "D", subject: "A" })).toBeNull();
    expect(navtexHeader({ station: null, subject: "A", serial: 7 })).toBeNull();
  });

  it("names only the things that went wrong", () => {
    expect(navtexQuality({ errors_corrected: 0, complete: true })).toBe("");
    expect(navtexQuality({ errors_corrected: 3, complete: true })).toBe("3 repaired");
    expect(navtexQuality({ errors_corrected: 0, complete: false })).toBe("truncated");
    expect(navtexQuality({ errors_corrected: 2, complete: false })).toBe("truncated · 2 repaired");
  });
});

describe("ACARS", () => {
  it("drops the flight number when the block has none", () => {
    expect(acarsHeadline({ registration: "D-AIBC", flight: "LH0400" })).toBe("D-AIBC · LH0400");
    expect(acarsHeadline({ registration: "D-AIBC" })).toBe("D-AIBC");
    expect(acarsHeadline({ registration: "D-AIBC", flight: "   " })).toBe("D-AIBC");
  });

  it("tags direction, a NAK and a continued block", () => {
    expect(acarsTag({ label: "H1", block_id: "3", downlink: true, ack: "C", more: false })).toBe(
      "H1 · DL",
    );
    expect(acarsTag({ label: "5Z", block_id: "K", downlink: false, ack: null, more: true })).toBe(
      "5Z · UL · NAK · more",
    );
  });
});

describe("sub-GHz", () => {
  it("describes a raw capture by its size rather than an empty payload", () => {
    expect(subghzPayload({ bits: 24, data: "0A1B23" })).toBe("0A1B23 (24 bit)");
    expect(subghzPayload({ bits: 0, data: "", timings_us: [320, 960, 320] })).toBe("raw, 3 edges");
    expect(subghzPayload({ bits: 0, data: "" })).toBe("raw, 0 edges");
  });

  it("offers only the readings the frame actually supports", () => {
    expect(subghzReadings({})).toBe("");
    expect(subghzReadings({ address: 0xa1b2, button: 3 })).toBe("addr 0A1B2 · btn 3");
    expect(subghzReadings({ tri_state: "01F01F01F01F" })).toBe("PT 01F01F01F01F");
  });

  it("reports the base period and only a repeat count above one", () => {
    expect(subghzTiming({ short_us: 320, repeats: 1 })).toBe("320 µs");
    expect(subghzTiming({ short_us: 320, repeats: 6 })).toBe("320 µs · ×6");
    expect(subghzTiming({ short_us: 0, repeats: 1 })).toBe("");
  });
});

describe("digital voice", () => {
  it("names the network the way each mode publishes it", () => {
    expect(dvNetwork({ mode: "dmr", color_code: 1, slot: 2 })).toBe("TS2 CC 1");
    expect(dvNetwork({ mode: "p25", color_code: 0x293, slot: null })).toBe("NAC 293");
    expect(dvNetwork({ mode: "nxdn", color_code: 5, slot: null })).toBe("RAN 5");
    expect(dvNetwork({ mode: "m17", color_code: null, slot: null })).toBe("");
  });

  it("marks a talkgroup so a number is not read as a radio", () => {
    expect(
      dvParties({
        source: 2621001,
        destination: 505,
        group_call: true,
        source_call: null,
        destination_call: null,
      }),
    ).toBe("TG 505 ← 2621001");
    expect(
      dvParties({
        source: 2621001,
        destination: 2621002,
        group_call: false,
        source_call: null,
        destination_call: null,
      }),
    ).toBe("2621002 ← 2621001");
  });

  it("prefers callsigns where the mode has them, and reports what it has", () => {
    expect(
      dvParties({
        source: null,
        destination: null,
        group_call: true,
        source_call: "DL1ABC",
        destination_call: "ALL",
      }),
    ).toBe("ALL ← DL1ABC");
    expect(
      dvParties({
        source: 42,
        destination: null,
        group_call: null,
        source_call: null,
        destination_call: null,
      }),
    ).toBe("42");
    expect(
      dvParties({
        source: null,
        destination: null,
        group_call: null,
        source_call: null,
        destination_call: null,
      }),
    ).toBe("");
  });

  it("reads a frame kind as a scanner would say it", () => {
    expect(dvKind({ kind: "header" })).toBe("call");
    expect(dvKind({ kind: "terminator" })).toBe("end");
    expect(dvKind({ kind: "control" })).toBe("signalling");
  });

  it("spells each mode as operators write it", () => {
    expect(dvMode({ mode: "dstar" })).toBe("D-STAR");
    expect(dvMode({ mode: "dpmr" })).toBe("dPMR");
  });
});
