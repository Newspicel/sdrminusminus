import { describe, expect, it } from "vitest";
import type { DeviceSet, ScannerStatus } from "../../lib/types";
import { refLabel, scannerOwnsTuning } from "./DeviceFace";

function deviceSet(overrides: Partial<DeviceSet> = {}): DeviceSet {
  return {
    id: 1,
    device: { driver: "virtual", key: "siggen", label: "Signal Generator" },
    capabilities: {
      freq_ranges: [],
      sample_rates: [],
      gains: [],
      antennas: [],
      bandwidths: [],
      extra: [],
      tx_capable: false,
    },
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
  });

  // A serial-less singleton is a legal reference (CANVAS §3), and it must still read as a radio
  // rather than as an empty separator.
  it("falls back to the backend alone", () => {
    expect(refLabel({ backend: "hackrf" })).toBe("hackrf");
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
