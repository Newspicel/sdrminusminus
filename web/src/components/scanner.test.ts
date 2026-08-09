import { describe, expect, it } from "vitest";
import type { DeviceSet, ScannerStatus, TemplateInfo } from "../lib/types";
import { rankDevices } from "./FirstRun";
import { formatDb, liveStatus, parseRanges, targetCount } from "./scanner";
import { reachable } from "./TemplatesPanel";

describe("parseRanges", () => {
  it("converts the MHz/kHz the user types into wire Hz", () => {
    const parsed = parseRanges([{ startMhz: "145.6", stopMhz: "145.8", stepKhz: "12.5" }]);
    expect(parsed).toEqual({
      ranges: [{ start_hz: 145_600_000, stop_hz: 145_800_000, step_hz: 12_500 }],
    });
  });

  it("explains the first unusable field instead of sending it", () => {
    expect(parseRanges([{ startMhz: "", stopMhz: "145.8", stepKhz: "12.5" }])).toMatch(/numbers/);
    expect(parseRanges([{ startMhz: "145.6", stopMhz: "145.8", stepKhz: "0" }])).toMatch(
      /greater than zero/,
    );
    expect(parseRanges([{ startMhz: "146", stopMhz: "145", stepKhz: "12.5" }])).toMatch(
      /below the start/,
    );
    expect(parseRanges([])).toMatch(/at least one range/);
  });

  it("names the offending line when there is more than one", () => {
    const parsed = parseRanges([
      { startMhz: "145.6", stopMhz: "145.8", stepKhz: "12.5" },
      { startMhz: "1", stopMhz: "2", stepKhz: "x" },
    ]);
    expect(parsed).toMatch(/^range 2: /);
  });
});

describe("targetCount", () => {
  it("counts inclusively, matching the server's expansion", () => {
    expect(targetCount([{ start_hz: 100, stop_hz: 200, step_hz: 50 }])).toBe(3);
    // A stop that does not land on a step boundary must not invent a target past it.
    expect(targetCount([{ start_hz: 100, stop_hz: 249, step_hz: 50 }])).toBe(3);
    expect(
      targetCount([
        { start_hz: 100, stop_hz: 200, step_hz: 50 },
        { start_hz: 0, stop_hz: 0, step_hz: 10 },
      ]),
    ).toBe(4);
  });
});

const STATUS: ScannerStatus = {
  state: "scanning",
  settings: {
    ranges: [],
    frequencies: [],
    threshold_db: -55,
    dwell_ms: 250,
    resume_ms: 1500,
    measure_bw_hz: 12_500,
  },
  targets: 10,
  current_hz: 145_500_000,
  sweeps: 0,
  hits: 0,
};

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

describe("liveStatus", () => {
  it("prefers the pushed update but falls back to the snapshot", () => {
    const set = deviceSet({ scanner: STATUS });
    expect(liveStatus(set, undefined)).toBe(STATUS);
    const pushed = { ...STATUS, current_hz: 146_000_000 };
    expect(liveStatus(set, pushed)).toBe(pushed);
  });

  // A pushed status outlives the scan that produced it; the snapshot is what says a scan
  // exists, so a stale push must never resurrect a stopped scan in the panel.
  it("reports nothing when the snapshot has no scan", () => {
    expect(liveStatus(deviceSet(), STATUS)).toBeNull();
    expect(liveStatus(null, STATUS)).toBeNull();
  });
});

describe("formatDb", () => {
  it("renders a dash rather than a bogus number for an absent level", () => {
    expect(formatDb(-31.5)).toBe("-31.5 dB");
    expect(formatDb(null)).toBe("—");
    expect(formatDb(undefined)).toBe("—");
    expect(formatDb(Number.NEGATIVE_INFINITY)).toBe("—");
  });
});

const TEMPLATE: TemplateInfo = {
  id: "adsb",
  name: "Aircraft",
  description: "",
  explainer: "",
  center_hz: 1_090_000_000,
  sample_rate: 2_000_000,
  channels: [],
  min_freq_hz: 1_090_000_000,
  max_freq_hz: 1_090_000_000,
};

describe("template reachability", () => {
  it("greys out what the open device cannot tune", () => {
    const rtl = deviceSet({
      capabilities: {
        ...deviceSet().capabilities,
        freq_ranges: [{ min: 24_000_000, max: 1_766_000_000 }],
      },
    });
    const hf = deviceSet({
      capabilities: {
        ...deviceSet().capabilities,
        freq_ranges: [{ min: 0, max: 30_000_000 }],
      },
    });
    expect(reachable(TEMPLATE, rtl)).toBe(true);
    expect(reachable(TEMPLATE, hf)).toBe(false);
  });

  // A device that advertises no ranges (the virtual siggen) must not have every template
  // greyed out — the engine is the authority, and it accepts them.
  it("assumes reachable when the device advertises no ranges", () => {
    expect(reachable(TEMPLATE, deviceSet())).toBe(true);
    expect(reachable(TEMPLATE, null)).toBe(true);
  });
});

describe("rankDevices", () => {
  it("puts real hardware above the virtual devices", () => {
    const ranked = rankDevices([
      { driver: "virtual", key: "siggen", label: "Signal Generator" },
      { driver: "rtlsdr", key: "0001", label: "RTL-SDR" },
      { driver: "virtual", key: "file:a", label: "A recording" },
      { driver: "hackrf", key: "abcd", label: "HackRF One" },
    ]);
    expect(ranked.map((d) => d.label)).toEqual([
      "HackRF One",
      "RTL-SDR",
      "A recording",
      "Signal Generator",
    ]);
  });
});
