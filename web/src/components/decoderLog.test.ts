import { describe, expect, it } from "vitest";
import type { DecodedState } from "../lib/decoded";
import type { DecodedRecord, DecoderEvent, DecoderLogEntry } from "../lib/types";
import {
  buildRows,
  collectLive,
  DECODER_KINDS,
  DEFAULT_LOG_FILTER,
  droppedNotice,
  eventStation,
  eventSummary,
  isFiltered,
  kindLabel,
  type LogFilter,
  liveRow,
  matchesFilter,
  NO_WIRES,
  sourceSet,
  storedRow,
  toQuery,
} from "./decoderLog";

const WIRED = sourceSet("0:0,1:0");

const adsb: DecoderEvent = {
  kind: "adsb",
  data: { icao: "3c6444", df: 17, callsign: " DLH123 ", altitude_ft: 35_000, raw: "8d3c6444" },
};

const ais: DecoderEvent = {
  kind: "ais",
  data: {
    mmsi: 211_234_560,
    msg_type: 1,
    ais_channel: "A",
    nmea: "!AIVDM,1,1,,A,x,0*00",
    name: " NORDLICHT ",
    lat: 53.551_2,
    lon: 9.993_7,
  },
};

function entry(over: Partial<DecoderLogEntry> = {}): DecoderLogEntry {
  return {
    id: 1,
    at: "2026-08-09T12:00:00Z",
    kind: "adsb",
    station: "3c6444",
    summary: "3c6444 · DLH123",
    freq_hz: 1_090_000_000,
    device_set: 0,
    channel: 0,
    event: adsb,
    ...over,
  };
}

function record(over: Partial<DecodedRecord> = {}): DecodedRecord {
  return {
    at: "2026-08-09T12:00:01Z",
    event: adsb,
    freq_hz: 1_090_000_000,
    device_set: 0,
    channel: 0,
    ...over,
  };
}

function filter(over: Partial<LogFilter> = {}): LogFilter {
  return { ...DEFAULT_LOG_FILTER, ...over };
}

describe("kind labels", () => {
  it("labels every decoder the wire union declares", () => {
    expect(DECODER_KINDS).toContain("adsb");
    for (const kind of DECODER_KINDS) {
      expect(kindLabel(kind)).not.toBe("");
    }
    expect(kindLabel("adsb")).toBe("ADS-B");
  });

  it("falls back for a kind this build does not know", () => {
    expect(kindLabel("dmr")).toBe("DMR");
  });
});

describe("toQuery", () => {
  const wires = { nodes: "channel:a1", sources: "0:1" };

  it("drops empty selects so a cleared filter is one query key, not two", () => {
    expect(toQuery(filter(), wires)).toEqual({ limit: 500, ...wires });
    expect(toQuery(filter({ q: "   " }), wires)).toEqual({ limit: 500, ...wires });
  });

  it("carries every set field, trimmed", () => {
    expect(toQuery(filter({ q: " nord ", limit: 100 }), wires)).toEqual({
      q: "nord",
      limit: 100,
      ...wires,
    });
  });

  it("sends an empty scope rather than omitting it", () => {
    expect(toQuery(filter(), NO_WIRES)).toEqual({ limit: 500, nodes: "", sources: "" });
  });
});

describe("isFiltered", () => {
  it("ignores the row limit", () => {
    expect(isFiltered(filter({ limit: 100 }))).toBe(false);
    expect(isFiltered(filter({ q: " x " }))).toBe(true);
  });
});

describe("matchesFilter", () => {
  it("searches station and summary case-insensitively", () => {
    expect(matchesFilter(record(), filter({ q: "DLH" }), WIRED)).toBe(true);
    expect(matchesFilter(record(), filter({ q: "3C6444" }), WIRED)).toBe(true);
    expect(matchesFilter(record(), filter({ q: "nordlicht" }), WIRED)).toBe(false);
  });

  it("drops a frame from a channel that is not wired in", () => {
    const scope = sourceSet("0:0");
    expect(matchesFilter(record(), filter(), scope)).toBe(true);
    expect(matchesFilter(record({ channel: 1 }), filter(), scope)).toBe(false);
    expect(matchesFilter(record({ device_set: 1 }), filter(), scope)).toBe(false);
    expect(matchesFilter(record(), filter(), sourceSet(""))).toBe(false);
  });
});

describe("collectLive", () => {
  const frames = {
    adsb: [record({ at: "2026-08-09T12:00:03Z" }), record({ at: "2026-08-09T12:00:01Z" })],
    ais: [record({ at: "2026-08-09T12:00:02Z", event: ais })],
  } as DecodedState["frames"];

  it("merges every decoder newest first", () => {
    expect(collectLive(frames, filter(), WIRED).map((r) => r.at)).toEqual([
      "2026-08-09T12:00:03Z",
      "2026-08-09T12:00:02Z",
      "2026-08-09T12:00:01Z",
    ]);
  });

  it("honours the filter and the cap", () => {
    expect(collectLive(frames, filter({ q: "nordlicht" }), WIRED)).toHaveLength(1);
    expect(collectLive(frames, filter(), WIRED, 2).map((r) => r.at)).toEqual([
      "2026-08-09T12:00:03Z",
      "2026-08-09T12:00:02Z",
    ]);
  });

  it("sorts an unstamped frame oldest instead of poisoning the order", () => {
    const broken = {
      ...frames,
      adsb: [...(frames.adsb ?? []), record({ at: "not a date" })],
    } as DecodedState["frames"];
    expect(collectLive(broken, filter(), WIRED).at(-1)?.at).toBe("not a date");
  });
});

describe("buildRows", () => {
  it("puts live rows above the stored page and marks them", () => {
    const rows = buildRows([entry()], [record()]);
    expect(rows.map((r) => r.live)).toEqual([true, false]);
    expect(rows[0]?.summary).toBe("3c6444 · DLH123 · 35000 ft");
  });

  it("orders the tail and the stored page as one table", () => {
    const rows = buildRows(
      [entry({ id: 2, at: "2026-08-09T12:00:04Z" }), entry({ at: "2026-08-09T12:00:00Z" })],
      [record({ at: "2026-08-09T12:00:05Z" }), record({ at: "2026-08-09T12:00:02Z" })],
    );
    expect(rows.map((r) => r.at)).toEqual([
      "2026-08-09T12:00:05Z",
      "2026-08-09T12:00:04Z",
      "2026-08-09T12:00:02Z",
      "2026-08-09T12:00:00Z",
    ]);
  });

  it("drops a live frame the stored page already carries", () => {
    const stored = entry({ at: "2026-08-09T12:00:01Z", summary: "3c6444 · DLH123 · 35000 ft" });
    expect(buildRows([stored], [record()])).toHaveLength(1);
    expect(buildRows([stored], [record()])[0]?.live).toBe(false);
  });

  it("keys rows uniquely even when two identical frames arrive at the same instant", () => {
    const rows = buildRows([entry(), entry({ id: 2 })], [record(), record()]);
    expect(new Set(rows.map((r) => r.key)).size).toBe(rows.length);
  });
});

describe("row projection", () => {
  it("keeps a stored row verbatim", () => {
    expect(storedRow(entry({ station: null }))).toMatchObject({
      key: "stored:1",
      kind: "adsb",
      station: null,
      summary: "3c6444 · DLH123",
      freqHz: 1_090_000_000,
      live: false,
    });
  });

  it("derives station and summary for a live row", () => {
    expect(liveRow(record({ event: ais }))).toMatchObject({
      kind: "ais",
      station: "211234560",
      summary: "211234560 · NORDLICHT · 53.5512, 9.9937",
      live: true,
    });
  });
});

describe("eventSummary", () => {
  it("matches the server's rendering per decoder", () => {
    expect(
      eventSummary({
        kind: "aprs",
        data: { source: "DL1ABC-9", destination: "APRS", info: "hi", tnc2: "DL1ABC-9>APRS:hi" },
      }),
    ).toBe("DL1ABC-9>APRS:hi");
    expect(
      eventSummary({
        kind: "aprs",
        data: {
          source: "DL1ABC-7",
          destination: "S32U6T",
          info: '`(_fn"Oj/',
          tnc2: 'DL1ABC-7>S32U6T:`(_fn"Oj/',
          mic_e_message: "Returning",
        },
      }),
    ).toBe('DL1ABC-7>S32U6T:`(_fn"Oj/ · Returning');
    expect(eventSummary({ kind: "rtty", data: { text: "CQ CQ" } })).toBe("CQ CQ");
    expect(
      eventSummary({
        kind: "sstv",
        data: {
          seq: 1,
          mode: "martin_m1",
          width: 320,
          height: 256,
          lines: 256,
          complete: true,
          duration_ms: 114_300,
        },
      }),
    ).toBe("Martin M1 \u00b7 320\u00d7256 \u00b7 complete in 114 s");
    expect(
      eventSummary({
        kind: "sstv",
        data: {
          seq: 2,
          mode: "robot36",
          width: 320,
          height: 240,
          lines: 96,
          complete: false,
          duration_ms: 14_500,
        },
      }),
    ).toBe("Robot 36 \u00b7 320\u00d7240 \u00b7 96 of 240 lines");
    expect(eventSummary({ kind: "tone", data: { ctcss_hz: 88.5, open: true } })).toBe(
      "CTCSS 88.5 Hz · open",
    );
    expect(eventSummary({ kind: "tone", data: { dcs_code: 23, open: false } })).toBe(
      "DCS 023 · muted",
    );
    expect(eventSummary({ kind: "tone", data: { open: false } })).toBe("no tone · muted");
    expect(eventSummary({ kind: "morse", data: { text: "SOS", wpm: 18 } })).toBe("SOS");
    expect(
      eventSummary({ kind: "rds", data: { block_errors: 0, groups: 10, pi: "D3C2", ps: "NDR2" } }),
    ).toBe("PI D3C2 · NDR2");
  });

  it("renders a toned page without text as address and function", () => {
    const tone: DecoderEvent = {
      kind: "pocsag",
      data: {
        address: 1_234_567,
        baud: 1200,
        errors_corrected: 0,
        function: 3,
        payload: "tone",
        text: "",
      },
    };
    expect(eventSummary(tone)).toBe("1234567 (3)");
    expect(eventSummary({ ...tone, data: { ...tone.data, text: "CALL 42" } })).toBe(
      "1234567: CALL 42",
    );
  });

  it("omits fields a frame does not carry", () => {
    expect(eventSummary({ kind: "adsb", data: { icao: "3c6444", df: 11, raw: "5d" } })).toBe(
      "3c6444",
    );
    expect(eventSummary({ kind: "rds", data: { block_errors: 3, groups: 0 } })).toBe("");
  });
});

describe("eventStation", () => {
  it("is null for the character-stream decoders", () => {
    expect(eventStation({ kind: "rtty", data: { text: "x" } })).toBeNull();
    expect(eventStation({ kind: "morse", data: { text: "x", wpm: 12 } })).toBeNull();
    expect(eventStation({ kind: "rds", data: { block_errors: 0, groups: 1 } })).toBeNull();
  });

  it("does not mistake a Selcall recipient for the transmitter", () => {
    const event: DecoderEvent = {
      kind: "selcall",
      data: { system: "ccir1", code: "12234", tone_ms: 100 },
    };
    expect(eventSummary(event)).toBe("CCIR-1 · 12234");
    expect(eventStation(event)).toBeNull();
  });
});

describe("droppedNotice", () => {
  it("stays silent only when nothing was lost", () => {
    expect(droppedNotice(0, 0)).toBeNull();
    expect(droppedNotice(1, 0)).toBe("1 live frame dropped");
    expect(droppedNotice(0, 12)).toBe("12 frames never reached the log");
    expect(droppedNotice(2, 12)).toBe("2 live frames dropped · 12 frames never reached the log");
  });
});

describe("clock and GNSS summaries", () => {
  it("keeps acquisition measurements visible in the live log", () => {
    const event: DecoderEvent = {
      kind: "gnss",
      data: {
        prn: 7,
        doppler_hz: 1000,
        code_phase_chips: 158.34,
        cn0_db_hz: 44.5,
      },
    };
    expect(eventSummary(event)).toBe("GPS PRN 7 · +1000 Hz · 44.5 dB-Hz · acquired");
    expect(eventStation(event)).toBe("GPS-7");
  });

  it("names the clock service beside its decoded civil time", () => {
    const event: DecoderEvent = {
      kind: "radio_clock",
      data: {
        standard: "dcf77",
        datetime: "2026-08-15T12:34:00+02:00",
        dst: true,
        leap_warning: false,
        symbols: "M000",
      },
    };
    expect(eventSummary(event)).toBe("DCF77 · 2026-08-15T12:34:00+02:00");
    expect(eventStation(event)).toBe("DCF77");
  });
});

describe("wave-2 summaries", () => {
  it("renders a NAVTEX broadcast as header, subject and one line of text", () => {
    expect(
      eventSummary({
        kind: "navtex",
        data: {
          station: "D",
          subject: "A",
          subject_name: "Navigational warning",
          serial: 7,
          text: "GALE WARNING\nGERMAN BIGHT",
          errors_corrected: 0,
          complete: true,
        },
      }),
    ).toBe("DA07 · Navigational warning · GALE WARNING GERMAN BIGHT");
  });

  it("renders an ACARS block as aircraft, flight, label and text", () => {
    const acars: DecoderEvent = {
      kind: "acars",
      data: {
        mode: "2",
        registration: "D-AIBC",
        label: "H1",
        block_id: "3",
        downlink: true,
        flight: "LH0400",
        text: "REPORT OK",
        more: false,
      },
    };
    expect(eventSummary(acars)).toBe("D-AIBC · LH0400 · [H1] · REPORT OK");
    expect(eventStation(acars)).toBe("D-AIBC");
    expect(eventSummary({ ...acars, data: { ...acars.data, text: "" } })).toBe(
      "D-AIBC · LH0400 · [H1]",
    );
  });

  it("renders a sub-GHz frame, and a raw capture by its edge count", () => {
    const frame: DecoderEvent = {
      kind: "subghz",
      data: {
        modulation: "ook",
        encoding: "pwm",
        bits: 24,
        data: "0A1B23",
        address: 0x0_a1b2,
        button: 3,
        short_us: 320,
        repeats: 6,
      },
    };
    expect(eventSummary(frame)).toBe("24 bit 0A1B23 · addr 0A1B2 · btn 3 · ×6");
    expect(eventStation(frame)).toBe("0A1B2");

    const raw: DecoderEvent = {
      kind: "subghz",
      data: {
        modulation: "fsk",
        encoding: "raw",
        bits: 0,
        data: "",
        short_us: 250,
        repeats: 1,
        timings_us: [320, 960, 320, 960],
      },
    };
    expect(eventSummary(raw)).toBe("raw, 4 edges");
    expect(eventStation(raw)).toBeNull();
  });

  it("gives the new kinds the names operators use for them", () => {
    expect(kindLabel("navtex")).toBe("NAVTEX");
    expect(kindLabel("acars")).toBe("ACARS");
    expect(kindLabel("subghz")).toBe("Sub-GHz");
    expect(DECODER_KINDS).toContain("navtex");
    expect(DECODER_KINDS).toContain("acars");
    expect(DECODER_KINDS).toContain("subghz");
  });
});
