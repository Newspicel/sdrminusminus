import { describe, expect, it } from "vitest";
import { agcTip } from "../../components/AgcAuto";
import type { Capabilities, DeviceSet, PatchGraph } from "../../lib/types";
import { mergeSettings } from "../../lib/useDevicePatch";
import {
  agcDelta,
  agcGainDb,
  autoTuning,
  bondSaid,
  clippingSaid,
  coherentLanes,
  faultSaid,
  hasLaneControls,
  hearing,
  laneAgc,
  lanesMerged,
  lockStream,
  refLabel,
  refusalSaid,
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

describe("lanesMerged", () => {
  it("draws lanes only when every stream tunes on its own", () => {
    const lanes = deviceSet({
      capabilities: capabilities({ rx_streams: 5, per_stream: { tuning: true, gain: true } }),
    });
    const shared = deviceSet({
      capabilities: capabilities({ rx_streams: 4, per_stream: { gain: true } }),
    });
    expect(lanesMerged(lanes)).toBe(true);
    expect(lanesMerged(shared)).toBe(false);
    expect(lanesMerged(deviceSet())).toBe(false);
  });
});

describe("hasLaneControls", () => {
  it("offers lane controls only for what a lane sets on its own", () => {
    const gain = { kind: "lna" as const, name: "LNA", range: { min: 0, max: 40 } };
    const perLane = { tuning: true, gain: true };
    expect(hasLaneControls(capabilities({ per_stream: perLane, gains: [gain] }))).toBe(true);
    expect(hasLaneControls(capabilities({ per_stream: perLane }))).toBe(false);
    expect(hasLaneControls(capabilities({ gains: [gain] }))).toBe(false);
    expect(
      hasLaneControls(capabilities({ per_stream: { antenna: true }, antennas: ["A", "B"] })),
    ).toBe(true);
  });
});

describe("bondSaid", () => {
  it("names the bond between lanes, and nothing for independent ones", () => {
    expect(bondSaid("time_sync")).toBe("Shared clock");
    expect(bondSaid("phase_coherent")).toBe("Phase coherent");
    expect(bondSaid("none")).toBeNull();
    expect(bondSaid(undefined)).toBeNull();
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

function refused(settings: string[]): NonNullable<DeviceSet["refused"]> {
  return { settings, error: "endpoint stalled" };
}

describe("refusalSaid", () => {
  it("names what the radio would not take", () => {
    expect(refusalSaid(deviceSet({ refused: refused(["frequency"]) }))).toBe(
      "Radio refused the new frequency",
    );
    expect(refusalSaid(deviceSet({ refused: refused(["frequency", "gain", "AGC"]) }))).toBe(
      "Radio refused the new frequency, gain and AGC",
    );
    expect(refusalSaid(deviceSet({ refused: refused([]) }))).toBe("Radio refused the change");
  });

  it("stays quiet when nothing was refused or the radio is faulted", () => {
    expect(refusalSaid(deviceSet())).toBeNull();
    expect(
      refusalSaid(deviceSet({ status: "error", error: "gone", refused: refused(["frequency"]) })),
    ).toBeNull();
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

describe("clippingSaid", () => {
  it("says nothing while no lane is at full scale", () => {
    expect(clippingSaid(deviceSet())).toBeNull();
  });

  it("names the clipping lanes by their IQ port on a multi-lane radio", () => {
    const set = deviceSet({ capabilities: capabilities({ rx_streams: 5 }), clipping: [0, 3] });
    expect(clippingSaid(set)).toBe("iq1, iq4");
  });

  it("just says yes on a single-lane radio", () => {
    expect(clippingSaid(deviceSet({ clipping: [0] }))).toBe("yes");
  });
});

describe("lane AGC", () => {
  const tuner = { name: "tuner", kind: "tuner" as const, range: { min: 0, max: 49.6 } };
  const bank = capabilities({
    agc: { kind: "switch" },
    gains: [tuner],
    rx_streams: 5,
    per_stream: { tuning: true, gain: true, agc: true },
  });

  it("switches one lane of a bank and the whole radio otherwise", () => {
    expect(agcDelta(bank, 3, { on: true })).toEqual({
      streams: [{ stream: 3, agc: { on: true } }],
    });
    expect(agcDelta(capabilities({ agc: { kind: "switch" } }), 0, { on: false })).toEqual({
      agc: { on: false },
    });
  });

  it("reads each lane's own switch over the radio's", () => {
    const set = deviceSet({
      capabilities: bank,
      settings: { agc: { on: false }, streams: [{ stream: 2, agc: { on: true } }] },
    });
    expect(laneAgc(set, 2).on).toBe(true);
    expect(laneAgc(set, 1).on).toBe(false);
  });

  it("shows the gain the AGC settled on only while it runs", () => {
    const set = deviceSet({
      capabilities: bank,
      settings: { streams: [{ stream: 1, agc: { on: true } }] },
      agc_gains: [
        { stream: 1, value_db: 28.0 },
        { stream: 0, value_db: 12.5 },
      ],
    });
    expect(agcGainDb(set, 1)).toBe(28);
    expect(agcGainDb(set, 0)).toBeNull();
  });
});

describe("coherentLanes", () => {
  it("names the lanes a coherent node or an Array uses", () => {
    const at = { x: 0, y: 0 };
    const graph: PatchGraph = {
      nodes: [
        { id: "kraken", kind: "device", data: {}, position: at },
        {
          id: "bench",
          kind: "array",
          data: { members: 1, coherence: "time_sync", shared_tuning: true },
          position: at,
        },
        { id: "fm", kind: "channel", data: {}, position: at },
      ] as PatchGraph["nodes"],
      edges: [
        { from: { node: "kraken", port: "iq2" }, to: { node: "bench", port: "iq" } },
        { from: { node: "kraken", port: "iq4" }, to: { node: "bench", port: "iq2" } },
        { from: { node: "kraken", port: "iq" }, to: { node: "fm", port: "iq" } },
      ],
    };
    expect([...coherentLanes(graph, "kraken")].toSorted((a, b) => a - b)).toEqual([1, 3]);
  });
});

describe("agcTip", () => {
  const set = deviceSet({
    capabilities: capabilities({
      agc: { kind: "switch" },
      gains: [{ name: "tuner", kind: "tuner", range: { min: 0, max: 49.6 } }],
    }),
    settings: { agc: { on: true } },
    agc_gains: [{ stream: 0, value_db: 28 }],
  });

  it("reads back the gain and advises fixed gain on coherent lanes without forcing it", () => {
    expect(agcTip(set, 0, false)).toBe("AGC on at 28.0 dB");
    expect(agcTip(set, 0, true)).toBe(
      "AGC on at 28.0 dB. Fixed gain keeps coherent lanes calibrated",
    );
    expect(agcTip(deviceSet({ settings: { agc: { on: false } } }), 0, true)).toBe(
      "AGC off, as coherent lanes want",
    );
  });
});
