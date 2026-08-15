import { describe, expect, it } from "vitest";
import type { Capabilities, DeviceSet, ScannerStatus } from "../../lib/types";
import { mergeSettings } from "../../lib/useDevicePatch";
import { refLabel, scannerOwnsTuning, tuneDelta, tunerDials } from "./deviceNode";

function capabilities(overrides: Partial<Capabilities> = {}): Capabilities {
  return {
    freq_ranges: [],
    sample_rates: [],
    gains: [],
    antennas: [],
    bandwidths: [],
    extra: [],
    duplex: "rx_only",
    ...overrides,
  };
}

function deviceSet(overrides: Partial<DeviceSet> = {}): DeviceSet {
  return {
    id: 1,
    device: { driver: "virtual", key: "siggen", label: "Signal Generator" },
    capabilities: capabilities(),
    settings: {},
    status: "running",
    channels: [],
    overruns: 0,
    ...overrides,
  };
}

const SCANNING: ScannerStatus = {
  state: "scanning",
  settings: { ranges: [], dwell_ms: 100, threshold_db: -60 },
  current_hz: 145_500_000,
  sweeps: 0,
  hits: 0,
  targets: 1,
};

describe("refLabel", () => {
  it("names the radio by whichever identity the reference carries", () => {
    expect(refLabel({ backend: "rtlsdr", serial: "00000001" })).toBe("rtlsdr · 00000001");
    expect(refLabel({ backend: "virtual", key: "siggen" })).toBe("virtual · siggen");
    expect(refLabel({ backend: "soapy", serial: "123456", key: "123456@DT" })).toBe(
      "soapy · 123456@DT",
    );
  });

  it("falls back to the backend alone", () => {
    expect(refLabel({ backend: "hackrf" })).toBe("hackrf");
  });
});

describe("tunerDials", () => {
  it("draws exactly one unlabelled dial for a single-stream radio", () => {
    const set = deviceSet({ settings: { center_hz: 100_000_000 } });
    expect(tunerDials(set)).toEqual([{ stream: 0, port: null, hz: 100_000_000 }]);
  });

  it("still draws one dial for a shared-tuning array, whatever its stream count", () => {
    const array4 = deviceSet({
      capabilities: capabilities({ rx_streams: 4, per_stream: { gain: true } }),
      settings: { center_hz: 433_920_000 },
    });
    expect(tunerDials(array4)).toEqual([{ stream: 0, port: null, hz: 433_920_000 }]);
  });

  it("draws one dial per stream, named for its IQ port, when the radio tunes per stream", () => {
    const set = deviceSet({
      capabilities: capabilities({
        rx_streams: 2,
        per_stream: { tuning: true, gain: true, antenna: true },
      }),
      settings: {
        center_hz: 100_000_000,
        streams: [{ stream: 1, center_hz: 433_920_000 }],
      },
    });
    expect(tunerDials(set)).toEqual([
      { stream: 0, port: "iq1", hz: 100_000_000 },
      { stream: 1, port: "iq2", hz: 433_920_000 },
    ]);
  });

  it("leaves a single-lane radio's dial unnamed even where tuning is per-stream", () => {
    const set = deviceSet({
      capabilities: capabilities({ rx_streams: 1, per_stream: { tuning: true } }),
      settings: { center_hz: 100_000_000 },
    });
    expect(tunerDials(set)).toEqual([{ stream: 0, port: null, hz: 100_000_000 }]);
  });
});

describe("tuneDelta", () => {
  it("retunes the whole radio when tuning is shared", () => {
    expect(tuneDelta(capabilities({ rx_streams: 4 }), 0, 145_500_000)).toEqual({
      center_hz: 145_500_000,
    });
  });

  it("tunes only the lane touched on a per-stream radio", () => {
    const caps = capabilities({ rx_streams: 2, per_stream: { tuning: true } });
    const delta = tuneDelta(caps, 1, 434_000_000);
    expect(delta).toEqual({ streams: [{ stream: 1, center_hz: 434_000_000 }] });

    const set = deviceSet({
      capabilities: caps,
      settings: {
        center_hz: 100_000_000,
        streams: [
          { stream: 0, center_hz: 101_000_000 },
          { stream: 1, center_hz: 433_920_000 },
        ],
      },
    });
    const retuned = { ...set, settings: mergeSettings(set.settings, delta) };
    expect(tunerDials(retuned)).toEqual([
      { stream: 0, port: "iq1", hz: 101_000_000 },
      { stream: 1, port: "iq2", hz: 434_000_000 },
    ]);
  });
});

describe("scannerOwnsTuning", () => {
  it("takes the dial while a scan runs", () => {
    expect(scannerOwnsTuning(deviceSet({ scanner: SCANNING }))).toBe(true);
    expect(scannerOwnsTuning(deviceSet({ scanner: { ...SCANNING, state: "holding" } }))).toBe(true);
  });

  it("gives it back when there is no scan, or the scan has faulted", () => {
    expect(scannerOwnsTuning(deviceSet())).toBe(false);
    expect(
      scannerOwnsTuning(deviceSet({ scanner: { ...SCANNING, error: "device stopped retuning" } })),
    ).toBe(false);
  });
});
