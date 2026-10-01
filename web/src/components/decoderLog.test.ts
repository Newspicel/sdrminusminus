import { describe, expect, it } from "vitest";
import type { DecodedState } from "../lib/decoded";
import type { DecodedRecord, DecoderEvent, DecoderLogEntry } from "../lib/types";
import {
  buildRows,
  clampColumnWidth,
  collectLive,
  DECODER_KINDS,
  DEFAULT_LOG_FILTER,
  defaultColumnWidths,
  droppedNotice,
  eventStation,
  eventSummary,
  FLEX_COLUMN,
  isFiltered,
  kindLabel,
  LOG_COLUMNS,
  type LogFilter,
  liveRow,
  logDownloads,
  MAX_COLUMN_WIDTH,
  MIN_COLUMN_WIDTH,
  matchesFilter,
  reachedSink,
  readColumnWidths,
  resizeColumn,
  storedRow,
  toQuery,
  totalColumnWidth,
  writeColumnWidths,
} from "./decoderLog";

const LOG = "decoder_log:1";

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
    sinks: [LOG, "export:1"],
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
  const wires = { sink: LOG, wired: true };

  it("drops empty selects so a cleared filter is one query key, not two", () => {
    expect(toQuery(filter(), wires)).toEqual({ limit: 500, sink: LOG });
    expect(toQuery(filter({ q: "   " }), wires)).toEqual({ limit: 500, sink: LOG });
  });

  it("carries every set field, trimmed", () => {
    expect(toQuery(filter({ q: " nord ", limit: 100 }), wires)).toEqual({
      q: "nord",
      limit: 100,
      sink: LOG,
    });
  });

  it("asks by the sink node even when nothing is wired in yet", () => {
    expect(toQuery(filter(), { sink: LOG, wired: false })).toEqual({ limit: 500, sink: LOG });
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
    expect(matchesFilter(record(), filter({ q: "DLH" }), LOG)).toBe(true);
    expect(matchesFilter(record(), filter({ q: "3C6444" }), LOG)).toBe(true);
    expect(matchesFilter(record(), filter({ q: "nordlicht" }), LOG)).toBe(false);
  });

  it("trusts the server's verdict on which sinks a frame reached", () => {
    expect(matchesFilter(record(), filter(), LOG)).toBe(true);
    expect(matchesFilter(record({ sinks: ["export:1"] }), filter(), LOG)).toBe(false);
    expect(matchesFilter(record({ sinks: [] }), filter(), LOG)).toBe(false);
    expect(reachedSink(record({ sinks: undefined }), LOG)).toBe(false);
  });
});

describe("collectLive", () => {
  const frames = {
    adsb: [record({ at: "2026-08-09T12:00:03Z" }), record({ at: "2026-08-09T12:00:01Z" })],
    ais: [record({ at: "2026-08-09T12:00:02Z", event: ais })],
  } as DecodedState["frames"];

  it("merges every decoder newest first", () => {
    expect(collectLive(frames, filter(), LOG).map((r) => r.at)).toEqual([
      "2026-08-09T12:00:03Z",
      "2026-08-09T12:00:02Z",
      "2026-08-09T12:00:01Z",
    ]);
  });

  it("honours the filter and the cap", () => {
    expect(collectLive(frames, filter({ q: "nordlicht" }), LOG)).toHaveLength(1);
    expect(collectLive(frames, filter(), LOG, 2).map((r) => r.at)).toEqual([
      "2026-08-09T12:00:03Z",
      "2026-08-09T12:00:02Z",
    ]);
  });

  it("sorts an unstamped frame oldest instead of poisoning the order", () => {
    const broken = {
      ...frames,
      adsb: [...(frames.adsb ?? []), record({ at: "not a date" })],
    } as DecodedState["frames"];
    expect(collectLive(broken, filter(), LOG).at(-1)?.at).toBe("not a date");
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
    expect(
      eventSummary({
        kind: "aprs",
        data: {
          source: "DL1WX",
          destination: "APRS",
          info: "_",
          tnc2: "DL1WX>APRS:_",
          weather: {
            temperature_c: 12.5,
            humidity_pct: 81,
            pressure_hpa: 1013.2,
            wind_dir_deg: 45,
            wind_speed_ms: 3.1,
            rain_1h_mm: 0.2,
          },
        },
      }),
    ).toBe("DL1WX · 12.5 °C · 81% · 1013.2 hPa · 045° 3.1 m/s · 0.2 mm/h");
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
    expect(
      eventSummary({
        kind: "apt",
        data: {
          seq: 1,
          lines: 1200,
          complete: true,
          duration_ms: 600_400,
          channel_a: "ch2",
          channel_b: "ch4",
        },
      }),
    ).toBe("APT · ch 2/4 · 1200 lines in 600 s");
    expect(
      eventSummary({
        kind: "lrpt",
        data: {
          seq: 1,
          mode: "oqpsk72",
          width: 1568,
          lines: 800,
          complete: false,
          duration_ms: 1,
          apids: [64, 65],
          frames: 10,
          frames_corrected: 1,
          frames_failed: 2,
          packets_lost: 3,
        },
      }),
    ).toBe("OQPSK 72k · APID 64/65 · 800 lines · 2 frames lost · 3 packets lost");
    expect(
      eventSummary({
        kind: "wefax",
        data: {
          seq: 1,
          ioc: "ioc576",
          lpm: "lpm120",
          width: 1810,
          lines: 400,
          complete: false,
          duration_ms: 1,
        },
      }),
    ).toBe("IOC 576 · 120 LPM · 400 lines, cut short");
    expect(
      eventSummary({
        kind: "radiosonde",
        data: {
          sonde: "rs41",
          serial: "S1234567",
          altitude_m: 12_345.4,
          climb_ms: 5.04,
          temperature_c: -40.04,
          lat: 48.1,
          lon: 11.5,
          errors_corrected: 0,
        },
      }),
    ).toBe("RS41 S1234567 · 12345 m · +5.0 m/s · -40.0 °C · 48.10000, 11.50000");
    expect(eventSummary({ kind: "tone", data: { ctcss_hz: 88.5, open: true } })).toBe(
      "CTCSS 88.5 Hz · open",
    );
    expect(eventSummary({ kind: "tone", data: { dcs_code: 23, open: false } })).toBe(
      "DCS 023 · muted",
    );
    expect(eventSummary({ kind: "tone", data: { open: false } })).toBe("no tone · muted");
    expect(
      eventSummary({ kind: "scrambler", data: { inversion_hz: 3_300, confidence: 0.82 } }),
    ).toBe("inversion 3300 Hz · 82% confidence");
    expect(eventSummary({ kind: "scrambler", data: { confidence: 0 } })).toBe("no inversion");
    expect(eventSummary({ kind: "morse", data: { text: "SOS", wpm: 18 } })).toBe("SOS");
    expect(
      eventSummary({
        kind: "rds",
        data: { block_errors: 0, blocks: 40, groups: 10, pi: "D3C2", ps: "NDR2" },
      }),
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

  it("renders FLEX and ERMES pages with their pager address", () => {
    const flex: DecoderEvent = {
      kind: "flex",
      data: {
        address: 123456,
        payload: "alpha",
        text: "CALL 42",
        baud: 3200,
        levels: 4,
        cycle: 2,
        frame: 17,
        phase: "C",
        errors_corrected: 1,
      },
    };
    const ermes: DecoderEvent = {
      kind: "ermes",
      data: {
        local_address: 45678,
        message_number: 3,
        payload: "numeric",
        text: "012345",
        urgent: true,
        alert: 2,
        errors_corrected: 0,
      },
    };
    expect(eventSummary(flex)).toBe("123456: CALL 42");
    expect(eventStation(flex)).toBe("123456");
    expect(eventSummary(ermes)).toBe("45678: 012345");
    expect(eventStation(ermes)).toBe("45678");
  });

  it("renders each CW skimmer signal with its passband offset and speed", () => {
    const spot: DecoderEvent = {
      kind: "cw_skimmer",
      data: { offset_hz: -742.4, text: "CQ W1AW", wpm: 23.6, snr_db: 14.2 },
    };
    expect(eventSummary(spot)).toBe("-742 Hz · 24 WPM · CQ W1AW");
    expect(eventStation(spot)).toBeNull();
  });

  it("omits fields a frame does not carry", () => {
    expect(eventSummary({ kind: "adsb", data: { icao: "3c6444", df: 11, raw: "5d" } })).toBe(
      "3c6444",
    );
    expect(eventSummary({ kind: "rds", data: { block_errors: 3, blocks: 3, groups: 0 } })).toBe("");
  });
});

describe("eventStation", () => {
  it("is null for the character-stream decoders", () => {
    expect(eventStation({ kind: "rtty", data: { text: "x" } })).toBeNull();
    expect(eventStation({ kind: "morse", data: { text: "x", wpm: 12 } })).toBeNull();
    expect(
      eventStation({ kind: "rds", data: { block_errors: 0, blocks: 4, groups: 1 } }),
    ).toBeNull();
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

  it("gives the new kinds the names operators use for them", () => {
    expect(kindLabel("navtex")).toBe("NAVTEX");
    expect(kindLabel("acars")).toBe("ACARS");
    expect(DECODER_KINDS).toContain("navtex");
    expect(DECODER_KINDS).toContain("acars");
  });
});

function fakeStorage(): Storage {
  const map = new Map<string, string>();
  return {
    get length() {
      return map.size;
    },
    clear: () => map.clear(),
    getItem: (key: string) => map.get(key) ?? null,
    key: (index: number) => [...map.keys()][index] ?? null,
    removeItem: (key: string) => map.delete(key),
    setItem: (key: string, value: string) => map.set(key, value),
  };
}

function useStorage(store: Storage | undefined): void {
  Object.defineProperty(globalThis, "localStorage", {
    value: store,
    configurable: true,
    writable: true,
  });
}

describe("column widths", () => {
  it("starts from the declared defaults", () => {
    const widths = defaultColumnWidths();
    expect(Object.keys(widths)).toEqual(LOG_COLUMNS.map((column) => column.key));
    expect(totalColumnWidth(widths)).toBe(
      LOG_COLUMNS.reduce((sum, column) => sum + column.width, 0),
    );
  });

  it("clamps to the allowed range and rounds to whole pixels", () => {
    expect(clampColumnWidth(MIN_COLUMN_WIDTH - 40)).toBe(MIN_COLUMN_WIDTH);
    expect(clampColumnWidth(MAX_COLUMN_WIDTH + 40)).toBe(MAX_COLUMN_WIDTH);
    expect(clampColumnWidth(120.4)).toBe(120);
    expect(clampColumnWidth(Number.NaN)).toBe(MIN_COLUMN_WIDTH);
  });

  it("resizes one column without touching the others", () => {
    const widths = defaultColumnWidths();
    const next = resizeColumn(widths, "station", 240);
    expect(next.station).toBe(240);
    expect(next.summary).toBe(widths.summary);
    expect(widths.station).toBe(defaultColumnWidths().station);
  });

  it("round-trips through storage", () => {
    useStorage(fakeStorage());
    writeColumnWidths(resizeColumn(defaultColumnWidths(), "kind", 200));
    expect(readColumnWidths().kind).toBe(200);
  });

  it("keeps the flex column off storage so it always fills the panel", () => {
    const store = fakeStorage();
    useStorage(store);
    store.setItem(
      "sdrmm.decoderLog.columns",
      JSON.stringify({ ...defaultColumnWidths(), [FLEX_COLUMN]: 900 }),
    );
    expect(readColumnWidths()[FLEX_COLUMN]).toBe(defaultColumnWidths()[FLEX_COLUMN]);
    expect(totalColumnWidth(readColumnWidths())).toBe(totalColumnWidth(defaultColumnWidths()));
    useStorage(undefined);
  });

  it("falls back to defaults on missing, corrupt or bogus storage", () => {
    useStorage(undefined);
    expect(readColumnWidths()).toEqual(defaultColumnWidths());

    const store = fakeStorage();
    useStorage(store);
    store.setItem("sdrmm.decoderLog.columns", "{not json");
    expect(readColumnWidths()).toEqual(defaultColumnWidths());

    store.setItem(
      "sdrmm.decoderLog.columns",
      JSON.stringify({ kind: "wide", station: 9000, bogus: 12 }),
    );
    const widths = readColumnWidths();
    expect(widths.kind).toBe(defaultColumnWidths().kind);
    expect(widths.station).toBe(MAX_COLUMN_WIDTH);
    expect(Object.keys(widths)).toEqual(LOG_COLUMNS.map((column) => column.key));

    useStorage(undefined);
  });
});

describe("logDownloads", () => {
  it("offers both formats over the same filter", () => {
    const choices = logDownloads({ limit: 200, sink: "export:1", q: "wx" });
    expect(choices.map((choice) => choice.label)).toEqual(["CSV", "JSON"]);
    for (const choice of choices) {
      expect(choice.href).toContain("sink=export%3A1");
      expect(choice.href).toContain("q=wx");
    }
    expect(choices[0]?.href).toContain("csv");
    expect(choices[1]?.href).toContain("json");
  });
});
