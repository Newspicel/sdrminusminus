import { describe, expect, it } from "vitest";
import { agcTip } from "../../components/AgcAuto";
import type { Capabilities, DeviceSet, PatchEdge, PatchGraph, PatchNode } from "../../lib/types";
import { mergeSettings } from "../../lib/useDevicePatch";
import { renderFace } from "../../test/faceHarness";
import { dialHold, heldLanes, laneHolds } from "./arrayNode";
import { DeviceFace } from "./DeviceFace";
import {
  agcDelta,
  agcGainDb,
  agcModeDelta,
  allAutoTuning,
  allLocked,
  autoTuning,
  bondSaid,
  clippingSaid,
  faultSaid,
  hearing,
  laneAgc,
  lanesAligned,
  lanesMerged,
  lockAll,
  lockStream,
  lossSaid,
  radioAgc,
  refLabel,
  refusalSaid,
  tuneAllDelta,
  tuneDelta,
  tunerDials,
  tuningAllDelta,
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
    device: { driver: "virtual", key: "band", label: "Test band" },
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
    expect(refLabel({ backend: "virtual", key: "band" })).toBe("virtual · band");
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

describe("tuneAllDelta", () => {
  it("moves every lane of a per-lane tuner to one frequency", () => {
    const caps = capabilities({
      rx_streams: 3,
      per_stream: { tuning: true },
      freq_ranges: [{ min: 24e6, max: 1766e6 }],
    });
    expect(tuneAllDelta(caps, 145.5e6)).toEqual({
      streams: [0, 1, 2].map((stream) => ({ stream, center_hz: 145.5e6, tuning: "manual" })),
    });
    expect(tuneAllDelta(caps, 1e6).streams?.[0]?.center_hz).toBe(24e6);
  });

  it("tunes a shared tuner in one field", () => {
    expect(tuneAllDelta(capabilities(), 100e6)).toEqual({ center_hz: 100e6, tuning: "manual" });
  });
});

describe("tuningAllDelta", () => {
  it("puts every lane, or the shared tuner, in the same mode", () => {
    const caps = capabilities({ rx_streams: 2, per_stream: { tuning: true } });
    expect(tuningAllDelta(caps, "auto")).toEqual({
      streams: [
        { stream: 0, tuning: "auto" },
        { stream: 1, tuning: "auto" },
      ],
    });
    expect(tuningAllDelta(capabilities(), "manual")).toEqual({ tuning: "manual" });
  });
});

describe("lanesAligned", () => {
  it("is true while every lane sits on one frequency", () => {
    const caps = capabilities({ rx_streams: 2, per_stream: { tuning: true } });
    const aligned = deviceSet({
      capabilities: caps,
      settings: {
        center_hz: 1e8,
        streams: [
          { stream: 0, center_hz: 1e8 },
          { stream: 1, center_hz: 1e8 },
        ],
      },
    });
    expect(lanesAligned(aligned)).toBe(true);
    expect(
      lanesAligned({
        ...aligned,
        settings: { ...aligned.settings, streams: [{ stream: 1, center_hz: 2e8 }] },
      }),
    ).toBe(false);
  });
});

describe("allAutoTuning", () => {
  it("needs every lane to follow the decoders", () => {
    const caps = capabilities({ rx_streams: 2, per_stream: { tuning: true } });
    expect(allAutoTuning(deviceSet({ capabilities: caps }))).toBe(true);
    expect(
      allAutoTuning(
        deviceSet({ capabilities: caps, settings: { streams: [{ stream: 1, tuning: "manual" }] } }),
      ),
    ).toBe(false);
  });
});

describe("lockAll", () => {
  it("holds or frees every lane at once", () => {
    const caps = capabilities({ rx_streams: 3 });
    expect(lockAll(caps, true)).toEqual([0, 1, 2]);
    expect(lockAll(caps, false)).toEqual([]);
    expect(allLocked([0, 1, 2], caps)).toBe(true);
    expect(allLocked([0, 2], caps)).toBe(false);
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
      "Test band is no longer attached. Plug it back in and it picks up where it left off.",
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

describe("lossSaid", () => {
  it("says how much of the stream is lost right now, and nothing once it stops", () => {
    expect(lossSaid(deviceSet({ loss: 0.74 }))).toBe("74%");
    expect(lossSaid(deviceSet({ overruns: 12 }))).toBeNull();
  });
});

describe("AGC modes across lanes", () => {
  const transceiver = capabilities({
    agc: {
      kind: "modes",
      options: [{ value: "fast_attack" }, { value: "slow_attack" }],
    },
    gains: [
      { name: "TUNER", kind: "tuner", range: { min: -3, max: 71 } },
      { name: "TX", kind: "tx", range: { min: -89.75, max: 0 }, agc: { kind: "never" } },
    ],
    rx_streams: 2,
    per_stream: { gain: true, agc: true },
  });

  it("shows the mode of a lane that runs its loop", () => {
    const set = deviceSet({
      capabilities: transceiver,
      settings: {
        agc: { on: false, mode: "fast_attack" },
        streams: [{ stream: 1, agc: { on: true, mode: "slow_attack" } }],
      },
    });
    expect(radioAgc(set)).toEqual({ on: true, mode: "slow_attack" });
  });

  it("sets a mode on every lane and leaves each one's switch alone", () => {
    const set = deviceSet({
      capabilities: transceiver,
      settings: { agc: { on: true }, streams: [{ stream: 1, agc: { on: false } }] },
    });
    expect(agcModeDelta(set, "slow_attack")).toEqual({
      streams: [
        { stream: 0, agc: { on: true, mode: "slow_attack" } },
        { stream: 1, agc: { on: false, mode: "slow_attack" } },
      ],
    });
  });

  it("reads back the gain of the one stage the loop drives", () => {
    const set = deviceSet({
      capabilities: transceiver,
      settings: { agc: { on: true } },
      agc_gains: [{ stream: 0, value_db: 44 }],
    });
    expect(agcGainDb(set, 0)).toBe(44);
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

function heldWires(count: number): PatchEdge[] {
  return Array.from({ length: count }, (_, lane) => ({
    from: { node: "kraken", port: lane === 0 ? "iq" : `iq${lane + 1}` },
    to: { node: "north", port: lane === 0 ? "lane" : `lane${lane + 1}` },
  }));
}

describe("arrays on a radio", () => {
  const radio: PatchNode = {
    id: "kraken",
    position: { x: 0, y: 0 },
    kind: "device",
    data: { device: { backend: "virtual", key: "kraken5" } },
  };
  const bank = deviceSet({
    device: { driver: "virtual", key: "kraken5", label: "KrakenSDR" },
    capabilities: capabilities({
      rx_streams: 5,
      coherence: "time_sync",
      freq_ranges: [{ min: 24_000_000, max: 1_766_000_000 }],
      gains: [{ name: "tuner", kind: "tuner", range: { min: 0, max: 49.6 } }],
      per_stream: { gain: true },
    }),
    settings: { center_hz: 145_000_000, sample_rate: 2_048_000 },
  });

  function patch(edges: PatchEdge[]): PatchGraph {
    const north = { id: "north", position: { x: 500, y: 0 }, kind: "array", label: "North" };
    return { nodes: [radio, north as PatchNode], edges };
  }

  it("offers Make array for a multi-lane radio with a shared clock", () => {
    const html = renderFace(DeviceFace, radio, {
      graph: patch([]),
      devices: new Map([["kraken", bank]]),
    });
    expect(html).toContain("Make array");
    expect(html).toContain('title="New Array wired to every lane"');
    const loose = { ...bank, capabilities: { ...bank.capabilities, coherence: "none" as const } };
    const noClock = renderFace(DeviceFace, radio, {
      graph: patch([]),
      devices: new Map([["kraken", loose]]),
    });
    expect(noClock).toContain('title="Lanes share no clock"');
    const single = renderFace(DeviceFace, radio, {
      graph: patch([]),
      devices: new Map([["kraken", deviceSet({ device: bank.device })]]),
    });
    expect(single).not.toContain("Make array");
  });

  it("disables held lane dials and says which array tunes them", () => {
    const graph = patch(heldWires(5));
    expect([...heldLanes(graph, "kraken")]).toEqual([
      [0, "north"],
      [1, "north"],
      [2, "north"],
      [3, "north"],
      [4, "north"],
    ]);
    const holds = laneHolds(graph, "kraken");
    expect(dialHold(holds, 0, false)).toEqual({ array: "north", label: "North" });
    expect(dialHold(laneHolds(patch(heldWires(1)), "kraken"), 1, true)).toBeNull();
    const free = renderFace(DeviceFace, radio, {
      graph: patch([]),
      devices: new Map([["kraken", bank]]),
    });
    expect(free).not.toContain('aria-disabled="true"');
    expect(free).not.toContain("Tuned by");
    const html = renderFace(DeviceFace, radio, { graph, devices: new Map([["kraken", bank]]) });
    expect(html).toContain('aria-disabled="true"');
    expect(html).toContain('title="Tuned by North"');
    expect(html).toContain('title="Set on North"');
    expect(html).not.toContain("Make array");
  });
});
