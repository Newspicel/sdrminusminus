import { describe, expect, it } from "vitest";
import type { DeviceSet, ScannerStatus, TemplateInfo } from "../lib/types";
import { rankDevices } from "./devices";
import {
  formatDb,
  formatMhz,
  gangCandidates,
  ganged,
  liveStatus,
  newRange,
  parseRanges,
  scanRefusal,
  sweepKind,
  targetCount,
} from "./scanner";
import { supports } from "./templates";

describe("parseRanges", () => {
  it("converts the MHz/kHz the editor holds into whole wire Hz", () => {
    expect(parseRanges([{ startMhz: 145.6, stopMhz: 145.8, stepKhz: 12.5 }])).toEqual({
      ranges: [{ start_hz: 145_600_000, stop_hz: 145_800_000, step_hz: 12_500 }],
    });
    const odd = parseRanges([{ startMhz: 433.075, stopMhz: 434.79, stepKhz: 8.33 }]);
    expect(odd).toEqual({
      ranges: [{ start_hz: 433_075_000, stop_hz: 434_790_000, step_hz: 8_330 }],
    });
  });

  it("refuses what no single field can catch", () => {
    expect(parseRanges([{ startMhz: 146, stopMhz: 145, stepKhz: 12.5 }])).toMatch(
      /below the start/,
    );
    expect(parseRanges([])).toMatch(/at least one range/);
  });

  it("names the offending line when there is more than one", () => {
    const parsed = parseRanges([
      { startMhz: 145.6, stopMhz: 145.8, stepKhz: 12.5 },
      { startMhz: 2, stopMhz: 1, stepKhz: 25 },
    ]);
    expect(parsed).toMatch(/^range 2: /);
  });
});

describe("newRange", () => {
  it("hands every row its own identity, so a removal cannot slide a draft onto its neighbour", () => {
    const rows = [newRange(), newRange(), newRange()];
    expect(new Set(rows.map((row) => row.id)).size).toBe(3);
    expect(rows[0]).toMatchObject({ startMhz: rows[1]?.startMhz, stopMhz: rows[1]?.stopMhz });
  });

  it("stays parseable — the id is editor state, not wire state", () => {
    expect(parseRanges([newRange()])).toEqual({
      ranges: [{ start_hz: 145_600_000, stop_hz: 145_800_000, step_hz: 12_500 }],
    });
  });
});

describe("targetCount", () => {
  it("counts inclusively, matching the server's expansion", () => {
    expect(targetCount([{ start_hz: 100, stop_hz: 200, step_hz: 50 }])).toBe(3);
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

  it("reports nothing when the snapshot has no scan", () => {
    expect(liveStatus(deviceSet(), STATUS)).toBeNull();
    expect(liveStatus(null, STATUS)).toBeNull();
  });
});

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

describe("formatMhz", () => {
  it("shows a dash rather than a frequency nobody reported", () => {
    expect(formatMhz(null)).toBe("—");
    expect(formatMhz(undefined)).toBe("—");
    expect(formatMhz(Number.NaN)).toBe("—");
    expect(formatMhz(145_500_000)).toBe("145.5000 MHz");
  });
});

describe("gangCandidates", () => {
  const base = {
    settings: {},
    channels: [],
    capabilities: {
      freq_ranges: [],
      sample_rates: [],
      gains: [],
      antennas: [],
      bandwidths: [],
      rx_streams: 1,
      tx_streams: 0,
      duplex: "rx_only",
    },
  } as unknown as DeviceSet;
  const set = (over: Partial<DeviceSet>): DeviceSet =>
    ({ ...base, status: "running", ...over }) as unknown as DeviceSet;

  it("offers only radios that are free to join the sweep", () => {
    const active = set({ id: 1 });
    const sets = [
      active,
      set({ id: 2 }),
      set({ id: 3, status: "idle" }),
      set({ id: 4, scanner: {} as never }),
      set({ id: 5, hunt: {} as never }),
      set({
        id: 6,
        capabilities: { ...base.capabilities, per_stream: { tuning: true } },
      }),
    ];
    expect(gangCandidates(sets, active).map((s) => s.id)).toEqual([2]);
  });

  it("offers nothing when no radio is driving the scan", () => {
    expect(gangCandidates([set({ id: 1 })], null)).toEqual([]);
  });
});

describe("ganged", () => {
  const active = { id: 1 } as DeviceSet;
  it("names the other radios sweeping the same plan", () => {
    const session = { device_sets: [1, 2, 3], settings: {} };
    expect(ganged(session, active)).toEqual([2, 3]);
  });

  it("says nothing when this radio is not in the scan", () => {
    expect(ganged(null, active)).toEqual([]);
    expect(ganged({ device_sets: [2], settings: {} }, active)).toEqual([]);
    expect(ganged({ device_sets: [1, 2], settings: {} }, null)).toEqual([]);
  });
});

describe("sweepKind", () => {
  it("says what is actually doing the sweeping, not what was asked for", () => {
    const radio = { capabilities: { hardware_sweep: true } } as DeviceSet;
    expect(sweepKind(radio, null)).toBe("the radio's own");
    expect(sweepKind(radio, { hardware_sweep: false } as never)).toBe("by retuning");
    expect(sweepKind(null, null)).toBe("by retuning");
  });
});
