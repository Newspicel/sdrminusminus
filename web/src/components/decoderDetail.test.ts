import { describe, expect, it } from "vitest";
import type { DecoderEvent, DecoderKind } from "../lib/types";
import { eventDetail } from "./decoderDetail";
import { DECODER_KINDS } from "./decoderLog";

function fieldsOf(event: DecoderEvent): Record<string, string> {
  return Object.fromEntries(eventDetail(event).fields);
}

describe("eventDetail", () => {
  it("answers for every decoder the wire union declares", () => {
    const sample: Record<DecoderKind, DecoderEvent> = {
      rds: { kind: "rds", data: { groups: 0, block_errors: 0 } },
      pocsag: {
        kind: "pocsag",
        data: {
          address: 1234,
          function: 0,
          baud: 512,
          payload: "tone",
          text: "",
          errors_corrected: 0,
        },
      },
      flex: {
        kind: "flex",
        data: {
          address: 123456,
          payload: "alpha",
          text: "TEST",
          baud: 1600,
          levels: 2,
          cycle: 1,
          frame: 2,
          phase: "A",
          errors_corrected: 0,
        },
      },
      ermes: {
        kind: "ermes",
        data: {
          local_address: 12345,
          message_number: 1,
          payload: "alpha",
          text: "TEST",
          urgent: false,
          alert: 0,
          errors_corrected: 0,
        },
      },
      adsb: { kind: "adsb", data: { icao: "3c6444", df: 17, raw: "8d" } },
      ais: { kind: "ais", data: { mmsi: 1, msg_type: 1, ais_channel: "A", nmea: "!AIVDM" } },
      aprs: { kind: "aprs", data: { source: "A", destination: "B", info: "", tnc2: "A>B:" } },
      rtty: { kind: "rtty", data: { text: "" } },
      morse: { kind: "morse", data: { text: "", wpm: 0 } },
      cw_skimmer: {
        kind: "cw_skimmer",
        data: { offset_hz: 750, text: "CQ", wpm: 18, snr_db: 12 },
      },
      ft8: {
        kind: "ft8",
        data: {
          text: "CQ W1AW FN42",
          snr_db: -10,
          audio_hz: 1_500,
          time_offset_s: 0.5,
          hard_errors: 0,
        },
      },
      ft4: {
        kind: "ft4",
        data: {
          text: "CQ JA1ABC PM95",
          snr_db: -8,
          audio_hz: 1_000,
          time_offset_s: 0.5,
          hard_errors: 0,
        },
      },
      psk31: { kind: "psk31", data: { text: "CQ TEST" } },
      psk63: { kind: "psk63", data: { text: "CQ TEST" } },
      wspr: {
        kind: "wspr",
        data: {
          text: "K1ABC FN42 37",
          callsign: "K1ABC",
          grid: "FN42",
          power_dbm: 37,
          snr_db: -20,
          audio_hz: 1_500,
          time_offset_s: 1,
          drift_hz: 0,
        },
      },
      selcall: { kind: "selcall", data: { system: "ccir1", code: "12345", tone_ms: 100 } },
      navtex: { kind: "navtex", data: { text: "", errors_corrected: 0, complete: true } },
      acars: {
        kind: "acars",
        data: {
          mode: "2",
          registration: "D-AIBC",
          label: "H1",
          block_id: "3",
          downlink: true,
          text: "",
          more: false,
        },
      },
      subghz: {
        kind: "subghz",
        data: {
          modulation: "ook",
          encoding: "raw",
          bits: 0,
          data: "",
          short_us: 0,
          repeats: 1,
        },
      },
      tone: { kind: "tone", data: { open: true } },
      dv: { kind: "dv", data: { mode: "dmr", kind: "header", errors_corrected: 0 } },
      ident: {
        kind: "ident",
        data: {
          modulation: "none",
          confidence: 1,
          bandwidth_hz: 0,
          center_offset_hz: 0,
          snr_db: 0,
          features: {
            envelope_variation: 0,
            duty: 0,
            keying_depth_db: 0,
            spectral_asymmetry: 0,
            carrier_db: 0,
            spectral_flatness: 0,
            frequency_levels: 0,
            frequency_spread_hz: 0,
            square_line_db: 0,
            quartic_line_db: 0,
          },
        },
      },
      broadcast: {
        kind: "broadcast",
        data: {
          system: "dab",
          locked: false,
          snr_db: 0,
          frequency_error_hz: 0,
        },
      },
      radio_clock: {
        kind: "radio_clock",
        data: {
          standard: "dcf77",
          datetime: "2026-08-15T12:34:00+02:00",
          dst: true,
          leap_warning: false,
          symbols: "M000",
        },
      },
      gnss: {
        kind: "gnss",
        data: {
          prn: 7,
          doppler_hz: 1000,
          code_phase_chips: 158.34,
          cn0_db_hz: 44.5,
        },
      },
    };
    for (const kind of DECODER_KINDS) {
      expect(() => eventDetail(sample[kind]), kind).not.toThrow();
    }
  });

  it("shows weak-signal timing and link measurements", () => {
    const detail = eventDetail({
      kind: "wspr",
      data: {
        text: "K1ABC FN42 37",
        callsign: "K1ABC",
        grid: "FN42",
        power_dbm: 37,
        snr_db: -21,
        audio_hz: 1_501.25,
        time_offset_s: 0.75,
        drift_hz: -0.2,
      },
    });
    expect(Object.fromEntries(detail.fields)).toMatchObject({
      Callsign: "K1ABC",
      Grid: "FN42",
      Power: "37 dBm",
      SNR: "-21 dB",
      Audio: "1501.3 Hz",
      "Time offset": "+0.75 s",
      Drift: "-0.2 Hz",
    });
    expect(detail.body).toBe("K1ABC FN42 37");
  });

  it("shows the Selcall plan, expanded code, and measured duration", () => {
    expect(
      fieldsOf({
        kind: "selcall",
        data: { system: "zvei1", code: "A11D0", tone_ms: 70 },
      }),
    ).toEqual({ "Tone plan": "ZVEI-1", Code: "A11D0", "Tone duration": "70 ms" });
  });

  it("omits the fields a frame did not carry rather than dashing them", () => {
    const bare = fieldsOf({ kind: "adsb", data: { icao: "3c6444", df: 11, raw: "5d" } });
    expect(bare).toEqual({ ICAO: "3C6444", "Downlink format": "11", Raw: "5d" });
    expect(bare).not.toHaveProperty("Callsign");

    const full = fieldsOf({
      kind: "adsb",
      data: {
        icao: "3c6444",
        df: 17,
        raw: "8d3c6444",
        callsign: " DLH123 ",
        altitude_ft: 37_000,
        ground_speed_kt: 451.4,
        track_deg: 271.6,
        vertical_rate_fpm: -1088,
        lat: 52.52,
        lon: 13.405,
      },
    });
    expect(full).toMatchObject({
      Callsign: "DLH123",
      Altitude: "37,000 ft",
      Position: "52.52000, 13.40500",
      "Ground speed": "451.4 kt",
      Track: "272°",
      "Vertical rate": "-1088 ft/min",
    });
  });

  it("pads a POCSAG RIC and names the function bit", () => {
    const detail = eventDetail({
      kind: "pocsag",
      data: {
        address: 1234,
        function: 3,
        baud: 1200,
        payload: "alpha",
        text: "CALL 42",
        errors_corrected: 2,
      },
    });
    expect(Object.fromEntries(detail.fields)).toMatchObject({
      RIC: "0001234",
      Function: "D (3)",
      Baud: "1200",
      Repaired: "2",
    });
    expect(detail.body).toBe("CALL 42");
  });

  it("keeps a NAVTEX broadcast's text intact, with the header it arrived under", () => {
    const detail = eventDetail({
      kind: "navtex",
      data: {
        station: "D",
        subject: "A",
        subject_name: "Navigational warning",
        serial: 7,
        text: "GALE WARNING\nGERMAN BIGHT",
        errors_corrected: 3,
        complete: false,
      },
    });
    expect(Object.fromEntries(detail.fields)).toMatchObject({
      Header: "DA07",
      Subject: "Navigational warning",
      Serial: "07",
      "Ended with NNNN": "no — flushed early",
      Repaired: "3 characters",
    });
    expect(detail.body).toBe("GALE WARNING\nGERMAN BIGHT");
  });

  it("keeps an ACARS body and names the direction and continuation", () => {
    const detail = eventDetail({
      kind: "acars",
      data: {
        mode: "2",
        registration: "D-AIBC",
        flight: "LH0400 ",
        label: "H1",
        block_id: "3",
        downlink: true,
        seq_no: "M01A",
        text: "POS N52.5 E013.4\nFL370",
        more: true,
      },
    });
    expect(Object.fromEntries(detail.fields)).toMatchObject({
      Registration: "D-AIBC",
      Flight: "LH0400",
      Direction: "downlink",
      Sequence: "M01A",
      Acknowledges: "NAK",
      Continues: "yes — another block follows",
    });
    expect(detail.body).toBe("POS N52.5 E013.4\nFL370");
  });

  it("pairs sub-GHz timings pulse-with-gap and offers only the readings the frame supports", () => {
    const detail = eventDetail({
      kind: "subghz",
      data: {
        modulation: "ook",
        encoding: "pwm",
        bits: 24,
        data: "A1B2C3",
        address: 0xa1b2,
        button: 3,
        short_us: 320,
        repeats: 6,
        timings_us: [320, 960, 960, 320, 320],
      },
    });
    expect(Object.fromEntries(detail.fields)).toMatchObject({
      Payload: "A1B2C3 (24 bit)",
      "EV1527 address": "0A1B2",
      "EV1527 button": "3",
      "Base period": "320 µs",
      Repeats: "×6",
    });
    expect(detail.fields.map(([label]) => label)).not.toContain("PT2262 tri-state");
    expect(detail.body).toBe("320/960  960/320  320");
  });

  it("reports a raw capture as timings with no payload", () => {
    const detail = eventDetail({
      kind: "subghz",
      data: { modulation: "ook", encoding: "raw", bits: 0, data: "", short_us: 0, repeats: 1 },
    });
    expect(detail.fields.map(([label]) => label)).toEqual(["Modulation", "Encoding"]);
    expect(detail.body).toBeNull();
  });

  it("distinguishes a frame that said 'not encrypted' from one that did not say", () => {
    const said = fieldsOf({
      kind: "dv",
      data: { mode: "dmr", kind: "header", errors_corrected: 0, encrypted: false },
    });
    expect(said.Encrypted).toBe("no");
    const silent = fieldsOf({
      kind: "dv",
      data: { mode: "dmr", kind: "header", errors_corrected: 0 },
    });
    expect(silent).not.toHaveProperty("Encrypted");
  });

  it("exposes DMR and P25 metadata and keeps packet data readable", () => {
    const detail = eventDetail({
      kind: "dv",
      data: {
        mode: "dmr",
        kind: "control",
        errors_corrected: 2,
        vendor: "hytera",
        manufacturer_id: 8,
        talker_alias: "Dispatcher",
        lat: 52.52,
        lon: 13.405,
        position_error_m: 20,
        channel: 407,
        emergency: true,
        algorithm_id: 5,
        key_id: 42,
        message_indicator: "001122334455667788",
        slot_activity: [{ slot: 2, activity: "group voice", destination_hash: 0xab }],
        data: "A1B2C3",
      },
    });
    expect(Object.fromEntries(detail.fields)).toMatchObject({
      Vendor: "Hytera (0x08)",
      "Talker alias": "Dispatcher",
      Position: "52.52000, 13.40500",
      "Position error": "≤ 20 m",
      Channel: "407",
      Emergency: "yes",
      "Slot activity": "TS2 group voice (hash 0xAB)",
      Algorithm: "0x05",
      "Key ID": "0x002A",
      "Message indicator": "001122334455667788",
      Repaired: "2 bits",
    });
    expect(detail.body).toBe("A1B2C3");
  });

  it("reads an RDS picture as its fields, with the radiotext as the body", () => {
    const detail = eventDetail({
      kind: "rds",
      data: {
        groups: 100,
        block_errors: 2,
        pi: "D389",
        ps: "RADIO 1 ",
        pty_name: "Pop Music",
        tp: true,
        music: false,
        alt_freqs_hz: [98_000_000, 100_500_000],
        radiotext: "Now playing something",
      },
    });
    expect(Object.fromEntries(detail.fields)).toMatchObject({
      PI: "D389",
      Station: "RADIO 1",
      "Programme type": "Pop Music",
      "Traffic programme": "yes",
      Content: "speech",
      "Alternative frequencies": "98.0 MHz, 100.5 MHz",
      Groups: "100",
      "Block errors": "2",
    });
    expect(detail.body).toBe("Now playing something");
  });

  it("shows a broadcast acquisition without inventing multiplex metadata", () => {
    const detail = eventDetail({
      kind: "broadcast",
      data: {
        system: "dvb_s2",
        locked: true,
        snr_db: 18.25,
        frequency_error_hz: -32.4,
        symbol_rate: 333_000,
      },
    });
    expect(Object.fromEntries(detail.fields)).toEqual({
      System: "DVB-S2",
      Lock: "locked",
      SNR: "18.3 dB",
      "Frequency error": "-32 Hz",
      "Symbol rate": "333000 Bd",
    });
    expect(detail.body).toBeNull();
  });

  it("carries the APRS fields the packet's monitor line packs away", () => {
    const detail = eventDetail({
      kind: "aprs",
      data: {
        source: "DL1ABC-9",
        destination: "S32U6T",
        path: ["WIDE1-1", "WIDE2-1"],
        info: '`(_fn"Oj/',
        tnc2: 'DL1ABC-9>S32U6T:`(_fn"Oj/',
        lat: 52.52,
        lon: 13.405,
        course_deg: 251,
        speed_kt: 20,
        altitude_ft: 1500,
        mic_e_message: "En Route",
      },
    });
    expect(Object.fromEntries(detail.fields)).toMatchObject({
      Source: "DL1ABC-9",
      Path: "WIDE1-1 → WIDE2-1",
      Position: "52.52000, 13.40500",
      Course: "251°",
      Speed: "20.0 kt",
      Altitude: "1,500 ft",
      "Mic-E message": "En Route",
    });
    expect(detail.body).toBe('DL1ABC-9>S32U6T:`(_fn"Oj/');
  });

  it("has nothing to add for a frame whose whole content is its summary", () => {
    const detail = eventDetail({ kind: "rtty", data: { text: "" } });
    expect(detail.fields).toEqual([]);
    expect(detail.body).toBeNull();
  });
});
