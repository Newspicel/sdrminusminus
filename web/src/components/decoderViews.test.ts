import { describe, expect, it } from "vitest";
import type { StationOf } from "../lib/decoded";
import type { DecodedRecordOf, DecoderEventOf, DecoderKind, RdsUpdate } from "../lib/types";
import {
  ageClass,
  aircraftRow,
  appendTranscript,
  aprsWeatherStations,
  BURST_DROP_M,
  buildTranscript,
  cwSignalRows,
  dvMode,
  dvNetwork,
  dvParties,
  formatAge,
  formatAltFreqs,
  formatAltitudeFt,
  formatBearing,
  formatClock,
  formatPosition,
  formatSpeedKt,
  inScope,
  isAtBottom,
  latestVorReadings,
  latestWpm,
  multiVorFix,
  ptyLabel,
  radiosondeLog,
  radiosondeStations,
  rdsPicture,
  rdsQuality,
  recordsInScope,
  shipRow,
  sortTargets,
  stationsInScope,
  TARGET_MAX_AGE_MS,
  TARGET_STALE_MS,
  toneLabel,
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

function vorReading(stationName: string, lat: number, lon: number, radial: number) {
  return {
    station: stationName,
    station_lat: lat,
    station_lon: lon,
    magnetic_declination_deg: 0,
    radial_deg: radial,
    variable_phase_deg: 0,
    reference_phase_deg: 0,
    signal_db: -12,
    confidence: 1,
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

describe("multi-VOR fixes", () => {
  it("intersects two non-parallel radials", () => {
    const records = [
      record("vor", vorReading("A", 0, 0, 45)),
      record("vor", vorReading("B", 0, 2, 315), { channel: 1 }),
    ];
    const fix = multiVorFix(records);
    expect(fix).not.toBeNull();
    expect(fix?.lat).toBeCloseTo(1, 3);
    expect(fix?.lon).toBeCloseTo(1, 3);
    expect(fix?.residualKm).toBeLessThan(0.001);
    expect(fix?.stations).toBe(2);
  });

  it("intersects radials across the antimeridian", () => {
    const records = [
      record("vor", vorReading("A", 0, 179.5, 45)),
      record("vor", vorReading("B", 0, -178.5, 315), { channel: 1 }),
    ];
    const fix = multiVorFix(records);
    expect(fix).not.toBeNull();
    expect(fix?.lat).toBeCloseTo(1, 3);
    expect(fix?.lon).toBeCloseTo(-179.5, 3);
    expect(fix?.residualKm).toBeLessThan(0.001);
  });

  it("uses the newest reading from each station", () => {
    const newest = record("vor", vorReading("A", 0, 0, 45), {
      at: "2026-08-09T12:00:01Z",
    });
    const older = record("vor", vorReading("A", 0, 0, 90));
    expect(latestVorReadings([older, newest])).toEqual([newest]);
    expect(multiVorFix([newest])).toBeNull();
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
    expect(row.label).toBe("-");
    expect(row.position).toBe("-");
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
      position: "-",
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
    expect(formatAltitudeFt(null)).toBe("-");
    expect(formatAltitudeFt(900)).toBe("900 ft");
    expect(formatAltitudeFt(37_000)).toBe("37,000 ft");
    expect(formatAltitudeFt(-1_200)).toBe("−1,200 ft");
    expect(formatSpeedKt(undefined)).toBe("-");
    expect(formatSpeedKt(12.6)).toBe("13 kt");
    expect(formatBearing(null)).toBe("");
    expect(formatBearing(359.7)).toBe("0°");
    expect(formatBearing(-90)).toBe("270°");
  });

  it("needs both halves of a position", () => {
    expect(formatPosition(52.52, null)).toBe("-");
    expect(formatPosition(52.52, 13.405)).toBe("52.52000, 13.40500");
  });

  it("renders a clock, and a placeholder for an unparsable stamp", () => {
    const local = new Date(2026, 7, 9, 12, 34, 56).toISOString();
    expect(formatClock(local)).toBe("12:34:56");
    expect(formatClock("not a date")).toBe("--:--:--");
  });
});

describe("RDS", () => {
  const base: RdsUpdate = { groups: 0, blocks: 0, block_errors: 0 };

  it("folds frames forward without a later frame erasing an earlier field", () => {
    const records = [
      record("rds", {
        ...base,
        groups: 100,
        blocks: 401,
        block_errors: 1,
        radiotext: "Now playing",
      }),
      record("rds", { ...base, groups: 50, blocks: 200, ps: "RADIO 1", pi: "D389" }),
    ];
    expect(rdsPicture(records)).toEqual({
      groups: 100,
      blocks: 401,
      block_errors: 1,
      ps: "RADIO 1",
      pi: "D389",
      radiotext: "Now playing",
    });
    expect(rdsPicture([])).toBeNull();
  });

  it("drops the frames a retune left behind instead of blending two stations", () => {
    const records = [
      record("rds", { ...base, groups: 4, blocks: 16, pi: "D392", ps: "WDR 2" }),
      record("rds", { ...base, groups: 900, blocks: 3600, pi: "D3A3", radiotext: "SWR3 news" }),
    ];
    expect(rdsPicture(records)?.radiotext).toBeUndefined();
    expect(rdsPicture(records)?.ps).toBe("WDR 2");
  });

  it("drops the frames of an earlier station that never sent a PI code", () => {
    const records = [
      record("rds", { ...base, groups: 4, blocks: 16, ps: "WDR 2" }),
      record("rds", { ...base, groups: 900, blocks: 3600, radiotext: "SWR3 news" }),
    ];
    expect(rdsPicture(records)?.radiotext).toBeUndefined();
  });

  it("grades block errors against every block the decoder read", () => {
    expect(rdsQuality(base).label).toBe("no lock");
    expect(rdsQuality({ ...base, groups: 1000, blocks: 4010, block_errors: 10 }).label).toBe(
      "good",
    );
    expect(rdsQuality({ ...base, groups: 1000, blocks: 4200, block_errors: 200 }).label).toBe(
      "fair",
    );
    expect(rdsQuality({ ...base, groups: 100, blocks: 600, block_errors: 200 }).label).toBe("poor");
    expect(rdsQuality({ ...base, groups: 1000, blocks: 4000, block_errors: 0 }).errorRate).toBe(0);
  });

  it("counts the blocks of half-received groups even before the wire reports them", () => {
    const partial = rdsQuality({ ...base, groups: 100, blocks: 0, block_errors: 200 });
    expect(partial.errorRate).toBeCloseTo(200 / 600, 6);
  });

  it("prefers the wire's PTY name and falls back to the code", () => {
    expect(ptyLabel({ ...base, pty: 10, pty_name: "Pop Music" })).toBe("Pop Music");
    expect(ptyLabel({ ...base, pty: 10 })).toBe("PTY 10");
    expect(ptyLabel(base)).toBe("-");
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

  it("groups CW spots by carrier and keeps the text in the order it arrived", () => {
    const rows = cwSignalRows([
      record("cw_skimmer", { offset_hz: 4_210, text: "K", wpm: 27, snr_db: 14 }),
      record("cw_skimmer", { offset_hz: -3_500, text: "DE DL1AAA ", wpm: 18, snr_db: 21 }),
      record("cw_skimmer", { offset_hz: 4_180, text: "CQ ", wpm: 26, snr_db: 15 }),
      record("cw_skimmer", { offset_hz: -3_480, text: "CQ ", wpm: 17, snr_db: 20 }),
    ]);
    expect(rows).toHaveLength(2);
    expect(rows[0]).toEqual({
      frequencyHz: 1_090_000_000 - 3_500,
      offsetHz: -3_500,
      wpm: 18,
      snrDb: 21,
      text: "CQ DE DL1AAA ",
    });
    expect(rows[1]).toEqual({
      frequencyHz: 1_090_000_000 + 4_210,
      offsetHz: 4_210,
      wpm: 27,
      snrDb: 14,
      text: "CQ K",
    });
  });

  it("keeps carriers a group apart separate and caps a long transcript", () => {
    const rows = cwSignalRows(
      [
        record("cw_skimmer", { offset_hz: 700, text: "TU", wpm: 20, snr_db: 9 }),
        record("cw_skimmer", { offset_hz: 600, text: "VVV", wpm: 21, snr_db: 8 }),
      ],
      4,
    );
    expect(rows.map((row) => row.offsetHz)).toEqual([600, 700]);
    expect(
      cwSignalRows(
        [record("cw_skimmer", { offset_hz: 0, text: "ABCDEF", wpm: 20, snr_db: 9 })],
        4,
      )[0]?.text,
    ).toBe("CDEF");
  });

  it("has no rows without spots", () => {
    expect(cwSignalRows([])).toEqual([]);
  });

  it("treats a near-bottom scroll as bottom", () => {
    expect(isAtBottom({ scrollTop: 900, scrollHeight: 1000, clientHeight: 100 })).toBe(true);
    expect(isAtBottom({ scrollTop: 895, scrollHeight: 1000, clientHeight: 100 })).toBe(true);
    expect(isAtBottom({ scrollTop: 500, scrollHeight: 1000, clientHeight: 100 })).toBe(false);
  });
});

describe("toneLabel", () => {
  it("names what is under the carrier the way a radio does", () => {
    expect(toneLabel({})).toBe("");
    expect(toneLabel({ ctcss_hz: 88.5 })).toBe("CTCSS 88.5 Hz");
    expect(toneLabel({ ctcss_hz: 100 })).toBe("CTCSS 100.0 Hz");
    expect(toneLabel({ dcs_code: 23 })).toBe("DCS 023");
    expect(toneLabel({ dcs_code: 754 })).toBe("DCS 754");
    expect(toneLabel({ ctcss_hz: 88.5, dcs_code: 23 })).toBe("CTCSS 88.5 Hz · DCS 023");
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

  it("spells each mode as operators write it", () => {
    expect(dvMode({ mode: "dstar" })).toBe("D-STAR");
    expect(dvMode({ mode: "dpmr" })).toBe("dPMR");
  });
});

function sondeAt(
  offsetS: number,
  data: Partial<DecoderEventOf<"radiosonde">["data"]>,
): DecodedRecordOf<"radiosonde"> {
  return record(
    "radiosonde",
    { sonde: "rs41", serial: "S1", errors_corrected: 0, ...data },
    { at: new Date(NOW + offsetS * 1000).toISOString(), freq_hz: 403_000_000 },
  );
}

describe("radiosondeStations", () => {
  it("keeps the latest value of each field per serial", () => {
    const sondes = radiosondeStations([
      sondeAt(2, { frame: 3, altitude_m: 1_200 }),
      sondeAt(1, { frame: 2, temperature_c: 5, humidity_pct: 70 }),
      sondeAt(0, { serial: "S2", sonde: "dfm", altitude_m: 300 }),
    ]);
    expect(sondes.map((s) => s.serial)).toEqual(["S1", "S2"]);
    expect(sondes[0]).toMatchObject({
      frame: 3,
      altitudeM: 1_200,
      temperatureC: 5,
      humidityPct: 70,
      frames: 2,
      burst: false,
    });
  });

  it("flags a burst once altitude falls well below the peak", () => {
    const peak = 30_000;
    const rising = radiosondeStations([
      sondeAt(1, { altitude_m: peak - BURST_DROP_M + 1 }),
      sondeAt(0, { altitude_m: peak }),
    ]);
    expect(rising[0]?.burst).toBe(false);
    const fallen = radiosondeStations([
      sondeAt(3, { altitude_m: 2_000 }),
      sondeAt(2, { altitude_m: peak - BURST_DROP_M - 1 }),
      sondeAt(1, { altitude_m: peak }),
      sondeAt(0, { altitude_m: 29_000 }),
    ]);
    expect(fallen[0]).toMatchObject({ burst: true, maxAltitudeM: peak, altitudeM: 2_000 });
  });
});

describe("radiosondeLog", () => {
  it("lists one sonde's newest frames first, up to the limit", () => {
    const records = [
      sondeAt(3, { frame: 4 }),
      sondeAt(2, { serial: "S2", frame: 9 }),
      sondeAt(1, { frame: 2 }),
      sondeAt(0, { frame: 1 }),
    ];
    expect(radiosondeLog(records, "S1", 2).map((r) => r.event.data.frame)).toEqual([4, 2]);
  });
});

function weatherAt(
  offsetS: number,
  source: string,
  weather: DecoderEventOf<"aprs">["data"]["weather"],
): DecodedRecordOf<"aprs"> {
  return record(
    "aprs",
    { source, destination: "APRS", info: "_", tnc2: `${source}>APRS:_`, weather },
    { at: new Date(NOW + offsetS * 1000).toISOString() },
  );
}

describe("aprsWeatherStations", () => {
  it("keeps latest values and extremes per station", () => {
    const stations = aprsWeatherStations([
      weatherAt(3, "WX1", { temperature_c: 14, wind_gust_ms: 3 }),
      weatherAt(2, "WX2", { temperature_c: 20 }),
      weatherAt(1, "WX1", { temperature_c: 9, wind_gust_ms: 11, pressure_hpa: 1010 }),
      weatherAt(0, "WX1", { temperature_c: 12, humidity_pct: 80 }),
    ]);
    expect(stations.map((s) => s.source)).toEqual(["WX1", "WX2"]);
    expect(stations[0]).toMatchObject({
      weather: { temperature_c: 14, wind_gust_ms: 3, pressure_hpa: 1010, humidity_pct: 80 },
      minTemperatureC: 9,
      maxTemperatureC: 14,
      maxGustMs: 11,
      reports: 3,
    });
  });

  it("skips packets without weather", () => {
    expect(aprsWeatherStations([weatherAt(0, "DL1ABC", null)])).toEqual([]);
  });

  it("leaves extremes empty when never reported", () => {
    expect(aprsWeatherStations([weatherAt(0, "WX", { humidity_pct: 50 })])[0]).toMatchObject({
      minTemperatureC: null,
      maxTemperatureC: null,
      maxGustMs: null,
    });
  });
});
