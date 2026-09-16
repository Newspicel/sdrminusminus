import { describe, expect, it } from "vitest";
import type { Capabilities, DeviceSet } from "../../lib/types";
import { mergeSettings } from "../../lib/useDevicePatch";
import {
  autoTuning,
  faultSaid,
  hearing,
  lockStream,
  refLabel,
  tuneDelta,
  tunerDials,
  tuningDelta,
} from "./deviceNode";

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
  it("retunes the whole radio when tuning is shared, and takes the wheel", () => {
    expect(tuneDelta(capabilities({ rx_streams: 4 }), 0, 145_500_000)).toEqual({
      center_hz: 145_500_000,
      tuning: "manual",
    });
  });

  it("tunes only the lane touched on a per-stream radio", () => {
    const caps = capabilities({ rx_streams: 2, per_stream: { tuning: true } });
    const delta = tuneDelta(caps, 1, 434_000_000);
    expect(delta).toEqual({
      streams: [{ stream: 1, center_hz: 434_000_000, tuning: "manual" }],
    });

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

describe("faultSaid", () => {
  it("says what an unplugged radio needs from the operator", () => {
    const set = deviceSet({
      status: "error",
      fault: "unplugged",
      error: "the radio is no longer attached (control transfer failed: device disconnected)",
    });
    expect(faultSaid(set)).toBe(
      "Signal Generator is no longer attached. Plug it back in and it picks up where it left off.",
    );
  });

  it("names the program holding a radio open", () => {
    const set = deviceSet({ status: "error", fault: "in_use", error: "busy" });
    expect(faultSaid(set)).toContain("open in another program");
  });

  it("points a permission fault at the hardware check", () => {
    const set = deviceSet({ status: "error", fault: "permissions", error: "EACCES" });
    expect(faultSaid(set)).toContain("Check hardware");
  });

  it("leaves a fault nobody can act on to its own message", () => {
    expect(faultSaid(deviceSet({ status: "error", fault: "other", error: "boom" }))).toBeNull();
    expect(faultSaid(deviceSet({ status: "error", error: "boom" }))).toBeNull();
  });
});

describe("autoTuning", () => {
  it("follows the decoders until the operator takes the wheel", () => {
    expect(autoTuning(deviceSet())).toBe(true);
    expect(autoTuning(deviceSet({ settings: { tuning: "auto" } }))).toBe(true);
    expect(autoTuning(deviceSet({ settings: { tuning: "manual" } }))).toBe(false);
  });

  it("answers per stream where each stream tunes apart", () => {
    const apart = deviceSet({
      capabilities: capabilities({ rx_streams: 2, per_stream: { tuning: true } }),
      settings: { streams: [{ stream: 1, tuning: "manual" }] },
    });
    expect(autoTuning(apart, 0)).toBe(true);
    expect(autoTuning(apart, 1)).toBe(false);
  });

  it("has one answer for a radio with one synthesizer", () => {
    const shared = deviceSet({
      capabilities: capabilities({ rx_streams: 2 }),
      settings: { streams: [{ stream: 1, tuning: "manual" }] },
    });
    expect(autoTuning(shared, 1)).toBe(true);
  });
});

describe("tuningDelta", () => {
  it("switches the whole radio where tuning is shared", () => {
    expect(tuningDelta(capabilities({ rx_streams: 2 }), 1, "manual")).toEqual({
      tuning: "manual",
    });
  });

  it("switches only the stream touched where each tunes apart", () => {
    const caps = capabilities({ rx_streams: 2, per_stream: { tuning: true } });
    expect(tuningDelta(caps, 1, "auto")).toEqual({ streams: [{ stream: 1, tuning: "auto" }] });
  });
});

describe("lockStream", () => {
  it("holds and frees one stream without touching the others", () => {
    expect(lockStream([], 1, true)).toEqual([1]);
    expect(lockStream([1], 0, true)).toEqual([0, 1]);
    expect(lockStream([0, 1], 0, false)).toEqual([1]);
    expect(lockStream([1], 1, true)).toEqual([1]);
  });
});

function carrying(out: boolean[], stream = 0): DeviceSet["channels"] {
  return out.map((out_of_band, id) => ({
    id,
    stream,
    out_of_band,
    settings: {
      frequency_hz: 100_000_000,
      params: { type: "nfm", settings: {} },
    } as DeviceSet["channels"][number]["settings"],
  }));
}

describe("hearing", () => {
  it("is green when the window holds every decoder", () => {
    expect(hearing(deviceSet())).toEqual({ heard: 0, total: 0, tone: "ok" });
    expect(hearing(deviceSet({ channels: carrying([false, false, false]) }))).toEqual({
      heard: 3,
      total: 3,
      tone: "ok",
    });
  });

  it("is yellow when the window misses some of them", () => {
    expect(hearing(deviceSet({ channels: carrying([false, true, true]) }))).toEqual({
      heard: 1,
      total: 3,
      tone: "warn",
    });
  });

  it("is red when the window misses all of them", () => {
    expect(hearing(deviceSet({ channels: carrying([true, true]) }))).toEqual({
      heard: 0,
      total: 2,
      tone: "danger",
    });
  });

  it("counts the misses of a radio the operator is tuning by hand", () => {
    const set = deviceSet({ settings: { tuning: "manual" }, channels: carrying([false, true]) });
    expect(hearing(set)).toEqual({ heard: 1, total: 2, tone: "warn" });
  });

  it("is red while the radio is faulted", () => {
    const set = deviceSet({ status: "error", channels: carrying([false, false]) });
    expect(hearing(set)).toEqual({ heard: 2, total: 2, tone: "danger" });
  });
});
