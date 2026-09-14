import { describe, expect, it } from "vitest";
import type { IdentReport, IdentSignal } from "../lib/types";
import { eventDetail } from "./decoderDetail";
import { eventStation, eventSummary, kindLabel } from "./decoderLog";
import {
  candidateScore,
  identMeasurements,
  identOverview,
  modulationLabel,
  signalFrequency,
} from "./decoderViews";

function signal(overrides: Partial<IdentSignal> = {}): IdentSignal {
  return {
    modulation: "fsk4",
    confidence: 0.86,
    frequency_hz: 446_006_340,
    center_offset_hz: 90,
    bandwidth_hz: 12_400,
    snr_db: 24.5,
    symbol_rate_hz: 4801,
    deviation_hz: 1938,
    candidates: [
      { name: "DMR", type_id: "dmr", score: 1, confirmed: true, why: "DMR frame sync found" },
      { name: "P25 Phase 1", type_id: "p25", score: 0.38, confirmed: false, why: "same waveform" },
    ],
    features: {
      envelope_variation: 0.03,
      duty: 0.51,
      keying_depth_db: 56,
      spectral_asymmetry: -0.1,
      carrier_db: 6.2,
      spectral_flatness: 0.5,
      frequency_levels: 4,
      frequency_spread_hz: 1433,
      square_line_db: 13,
      quartic_line_db: 10,
    },
    ...overrides,
  };
}

function report(overrides: Partial<IdentSignal> = {}): IdentReport {
  return { snr_db: 24.5, signals: [signal(overrides)] };
}

const quiet: IdentReport = { snr_db: 4.1, signals: [] };

describe("modulationLabel", () => {
  it("names the family an operator would recognise", () => {
    expect(modulationLabel(signal())).toBe("4-FSK");
    expect(modulationLabel(signal({ modulation: "ofdm" }))).toBe("OFDM");
  });

  it("names the sideband when the identifier found one", () => {
    expect(modulationLabel(signal({ modulation: "ssb", sideband: "usb" }))).toBe("SSB (USB)");
  });
});

describe("signalFrequency", () => {
  it("places a signal on the dial to the hundred hertz", () => {
    expect(signalFrequency(signal())).toBe("446.0063 MHz");
    expect(signalFrequency(signal({ frequency_hz: 77_500 }))).toBe("77.50 kHz");
  });
});

describe("identMeasurements", () => {
  it("quotes only what was measured", () => {
    const fields = Object.fromEntries(identMeasurements(signal()));
    expect(fields).toMatchObject({
      Bandwidth: "12.4 kHz",
      "Symbol rate": "4801 Bd",
      Deviation: "±1938 Hz",
      Duty: "51%",
    });

    const noClock = Object.fromEntries(
      identMeasurements(
        signal({
          symbol_rate_hz: undefined,
          deviation_hz: undefined,
          features: { ...signal().features, duty: 1 },
        }),
      ),
    );
    expect(noClock).not.toHaveProperty("Symbol rate");
    expect(noClock).not.toHaveProperty("Deviation");
    expect(noClock).not.toHaveProperty("Duty");
  });

  it("spells out bursts and OFDM timing when they were measured", () => {
    const fields = Object.fromEntries(
      identMeasurements(
        signal({
          burst_ms: 29.9,
          burst_period_ms: 60,
          ofdm_symbol_us: 1000,
          ofdm_guard_us: 246,
        }),
      ),
    );
    expect(fields).toMatchObject({
      Bursts: "29.90 ms every 60.0 ms",
      "OFDM symbol": "1000 µs, guard 246 µs",
    });
  });

  it("reports how close an empty channel came to the threshold", () => {
    expect(Object.fromEntries(identOverview(quiet))).toEqual({
      "Loudest bin": "4.1 dB over the noise floor",
    });
    expect(identOverview(report())).toEqual([]);
  });
});

describe("candidateScore", () => {
  it("distinguishes a confirmed match from a resemblance", () => {
    expect(candidateScore({ score: 1, confirmed: true })).toBe("confirmed");
    expect(candidateScore({ score: 0.38, confirmed: false })).toBe("38%");
  });
});

describe("the decoder log", () => {
  it("labels the kind", () => {
    expect(kindLabel("ident")).toBe("Signal ID");
  });

  it("summarises a report on one line, best candidate last", () => {
    expect(eventSummary({ kind: "ident", data: report() })).toBe(
      "4-FSK · 12.4 kHz · 4801 Bd · ±1938 Hz · DMR (confirmed)",
    );
    expect(eventSummary({ kind: "ident", data: quiet })).toBe("no signal");
  });

  it("counts the signals of a survey before describing the loudest", () => {
    const survey: IdentReport = { snr_db: 30, signals: [signal(), signal({ modulation: "fm" })] };
    expect(eventSummary({ kind: "ident", data: survey })).toMatch(/^2 signals · 4-FSK/);
  });

  it("names no station: which transmitter it is, is the open question", () => {
    expect(eventStation({ kind: "ident", data: report() })).toBeNull();
  });

  it("expands to the measurements and the whole shortlist", () => {
    const detail = eventDetail({ kind: "ident", data: report() });
    expect(Object.fromEntries(detail.fields)).toMatchObject({
      Signals: "1",
      Frequency: "446.0063 MHz",
      Modulation: "4-FSK",
      Confidence: "86%",
      "Frequency levels": "4",
    });
    expect(detail.body).toContain("DMR — confirmed");
    expect(detail.body).toContain("P25 Phase 1 — 38%");
  });
});
