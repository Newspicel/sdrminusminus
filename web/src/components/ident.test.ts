import { describe, expect, it } from "vitest";
import type { IdentReport } from "../lib/types";
import { eventDetail } from "./decoderDetail";
import { eventStation, eventSummary, kindLabel } from "./decoderLog";
import { candidateScore, identMeasurements, modulationLabel } from "./decoderViews";

function report(overrides: Partial<IdentReport> = {}): IdentReport {
  return {
    modulation: "fsk4",
    confidence: 0.86,
    bandwidth_hz: 12_400,
    center_offset_hz: 90,
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

const quiet = report({
  modulation: "none",
  confidence: 1,
  bandwidth_hz: 0,
  center_offset_hz: 0,
  snr_db: 4.1,
  symbol_rate_hz: undefined,
  deviation_hz: undefined,
  candidates: [],
});

describe("modulationLabel", () => {
  it("names the family an operator would recognise", () => {
    expect(modulationLabel(report())).toBe("4-FSK");
    expect(modulationLabel(quiet)).toBe("no signal");
  });

  it("names the sideband when the identifier found one", () => {
    expect(modulationLabel(report({ modulation: "ssb", sideband: "usb" }))).toBe("SSB (USB)");
  });
});

describe("identMeasurements", () => {
  it("quotes only what was measured", () => {
    const fields = Object.fromEntries(identMeasurements(report()));
    expect(fields).toMatchObject({
      Bandwidth: "12.4 kHz",
      "Symbol rate": "4801 Bd",
      Deviation: "±1938 Hz",
      Duty: "51%",
    });

    const noClock = Object.fromEntries(
      identMeasurements(
        report({
          symbol_rate_hz: undefined,
          deviation_hz: undefined,
          features: { ...report().features, duty: 1 },
        }),
      ),
    );
    expect(noClock).not.toHaveProperty("Symbol rate");
    expect(noClock).not.toHaveProperty("Deviation");
    expect(noClock).not.toHaveProperty("Duty");
  });

  it("reports how close an empty channel came to the threshold", () => {
    expect(Object.fromEntries(identMeasurements(quiet))).toEqual({
      "Loudest bin": "4.1 dB over the noise floor",
    });
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

  it("names no station: which transmitter it is, is the open question", () => {
    expect(eventStation({ kind: "ident", data: report() })).toBeNull();
  });

  it("expands to the measurements and the whole shortlist", () => {
    const detail = eventDetail({ kind: "ident", data: report() });
    expect(Object.fromEntries(detail.fields)).toMatchObject({
      Modulation: "4-FSK",
      Confidence: "86%",
      "Frequency levels": "4",
    });
    expect(detail.body).toContain("DMR — confirmed");
    expect(detail.body).toContain("P25 Phase 1 — 38%");
  });
});
