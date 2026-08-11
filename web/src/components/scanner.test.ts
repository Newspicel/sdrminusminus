import { describe, expect, it } from "vitest";
import type { DeviceSet, ScannerStatus, TemplateInfo } from "../lib/types";
import { rankDevices } from "./OpenRadio";
import { formatDb, liveStatus, parseRanges, scanRefusal, targetCount } from "./scanner";
import { supports } from "./TemplatesPanel";

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
      duplex: "rx_only",
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

// Mirrors the server's refusal (a sweep needs one radio-wide tuning to own): the panel disables
// the start and says why, so the operator never sees the raw 400.
describe("scanRefusal", () => {
  it("refuses a radio whose streams tune independently", () => {
    const perStream = deviceSet({
      capabilities: {
        ...deviceSet().capabilities,
        rx_streams: 2,
        per_stream: { tuning: true, gain: true, antenna: true },
      },
    });
    expect(scanRefusal(perStream)).toMatch(/independently/);
  });

  it("allows a single-stream radio, a shared-tuning array, and no radio at all", () => {
    expect(scanRefusal(deviceSet())).toBeNull();
    const array4 = deviceSet({
      capabilities: { ...deviceSet().capabilities, rx_streams: 4, per_stream: { gain: true } },
    });
    expect(scanRefusal(array4)).toBeNull();
    expect(scanRefusal(null)).toBeNull();
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
  direction: "rx",
  exact_rate: true,
  supported_devices: ["rtlsdr:00000001"],
};

// Which radios can run a template is decided once, in `TemplateInfo::unmet_by`, and served as
// `supported_devices`. The panel looks the open radio up in that list rather than re-deriving
// the rule from the capability set — these cases pin the lookup, not the rule.
describe("template support", () => {
  it("offers the template on a radio the server listed", () => {
    const rtl = deviceSet({
      device: { driver: "rtlsdr", key: "00000001", label: "RTL-SDR" },
    });
    expect(supports(TEMPLATE, rtl)).toBe(true);
  });

  it("refuses a radio the server left out, and refuses with no radio open", () => {
    const other = deviceSet({
      device: { driver: "rtlsdr", key: "00000002", label: "RTL-SDR" },
    });
    expect(supports(TEMPLATE, other)).toBe(false);
    expect(supports(TEMPLATE, null)).toBe(false);
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
