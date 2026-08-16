import { describe, expect, it } from "vitest";
import type { ChannelInfo, DeviceInfo, DeviceSet, PatchGraph, PatchNode } from "../lib/types";
import {
  bindChannels,
  bindDevices,
  channelNodesOf,
  deviceNodeOf,
  deviceRefOf,
  inputsOf,
  iqSourceOf,
  refFromDeviceId,
  refMatches,
  unboundChannels,
} from "./binding";

function info(overrides: Partial<DeviceInfo>): DeviceInfo {
  return { driver: "rtlsdr", key: "0", label: "RTL-SDR", ...overrides };
}

function channel(id: number, type: string, stream = 0): ChannelInfo {
  return { id, stream, settings: { offset_hz: 0, params: { type, settings: {} } as never } };
}

function set(id: number, device: DeviceInfo, channels: ChannelInfo[] = []): DeviceSet {
  return {
    id,
    device,
    capabilities: {
      freq_ranges: [],
      sample_rates: [],
      gains: [],
      antennas: [],
      bandwidths: [],
      duplex: "rx_only",
    },
    settings: {},
    status: "running",
    channels,
    overruns: 0,
  };
}

function node(id: string, body: Partial<PatchNode> & Pick<PatchNode, "kind">): PatchNode {
  return { id, position: { x: 0, y: 0 }, ...body } as PatchNode;
}

describe("device references", () => {
  it("prefers the serial, falls back to the key, and accepts a singleton backend", () => {
    const hardware = info({ serial: "00000001" });
    const bySerial = deviceRefOf(hardware);
    expect(bySerial).toEqual({ backend: "rtlsdr", serial: "00000001" });
    expect(refMatches(bySerial, hardware)).toBe(true);
    expect(refMatches(bySerial, info({ key: "3", serial: "00000001" }))).toBe(true);
    expect(refMatches(bySerial, info({ serial: "00000002" }))).toBe(false);

    const duo = info({
      driver: "soapy",
      key: "123456@DT",
      label: "Dual Tuner",
      serial: "123456",
    });
    const byVariant = deviceRefOf(duo);
    expect(byVariant).toEqual({ backend: "soapy", serial: "123456", key: "123456@DT" });
    expect(refMatches(byVariant, duo)).toBe(true);
    expect(refMatches(byVariant, { ...duo, key: "123456@ST" })).toBe(false);

    const file = info({ driver: "virtual", key: "file:/rec/capture", serial: undefined });
    const byKey = deviceRefOf(file);
    expect(byKey).toEqual({ backend: "virtual", key: "file:/rec/capture" });
    expect(refMatches(byKey, file)).toBe(true);
    expect(refMatches(byKey, info({ driver: "virtual", key: "siggen" }))).toBe(false);

    const singleton = { backend: "hackrf" };
    expect(refMatches(singleton, info({ driver: "hackrf", key: "0" }))).toBe(true);
    expect(refMatches(singleton, hardware)).toBe(false);
  });
});

describe("binding", () => {
  const rtl = info({ serial: "A" });
  const other = info({ serial: "B" });

  function graph(): PatchGraph {
    return {
      nodes: [
        node("dev", { kind: "device", data: { device: deviceRefOf(rtl) } }),
        node("nfm", { kind: "channel", data: { channel_type: "nfm" } }),
        node("am", { kind: "channel", data: { channel_type: "am" } }),
        node("spk", { kind: "speaker" }),
      ],
      edges: [
        { from: { node: "dev", port: "iq" }, to: { node: "nfm", port: "iq" } },
        { from: { node: "dev", port: "iq" }, to: { node: "am", port: "iq" } },
        { from: { node: "nfm", port: "audio" }, to: { node: "spk", port: "audio" } },
      ],
    };
  }

  it("binds a device node to the set running its radio", () => {
    const devices = bindDevices(graph(), [set(2, other), set(1, rtl)]);
    expect(devices.get("dev")?.id).toBe(1);
    expect(bindDevices(graph(), [set(2, other)]).size).toBe(0);
  });

  it("claims each set once, in node order", () => {
    const clone = info({ driver: "rtlsdr", key: "0", serial: undefined });
    const twoNodes: PatchGraph = {
      nodes: [
        node("first", { kind: "device", data: { device: { backend: "rtlsdr" } } }),
        node("second", { kind: "device", data: { device: { backend: "rtlsdr" } } }),
      ],
      edges: [],
    };
    const bound = bindDevices(twoNodes, [set(1, clone), set(2, info({ key: "1" }))]);
    expect(bound.get("first")?.id).toBe(1);
    expect(bound.get("second")?.id).toBe(2);
  });

  it("binds channel nodes by type in stored order", () => {
    const live = set(1, rtl, [channel(7, "am"), channel(9, "nfm"), channel(11, "nfm")]);
    const devices = bindDevices(graph(), [live]);
    const channels = bindChannels(graph(), devices);
    expect(channels.get("nfm")?.id).toBe(9);
    expect(channels.get("am")?.id).toBe(7);
    expect(unboundChannels(graph(), "dev", live, channels).map((c) => c.id)).toEqual([11]);
  });

  it("scopes the orphan list to the set's own nodes", () => {
    const a = set(1, rtl, [channel(1, "nfm")]);
    const b = set(2, other, [channel(1, "am")]);
    const twoRadios: PatchGraph = {
      nodes: [
        node("devA", { kind: "device", data: { device: deviceRefOf(rtl) } }),
        node("nfm", { kind: "channel", data: { channel_type: "nfm" } }),
        node("devB", { kind: "device", data: { device: deviceRefOf(other) } }),
      ],
      edges: [{ from: { node: "devA", port: "iq" }, to: { node: "nfm", port: "iq" } }],
    };
    const devices = bindDevices(twoRadios, [a, b]);
    const channels = bindChannels(twoRadios, devices);
    expect(unboundChannels(twoRadios, "devA", a, channels)).toEqual([]);
    expect(unboundChannels(twoRadios, "devB", b, channels).map((c) => c.id)).toEqual([1]);
  });

  it("leaves a node unbound when the engine has no channel of its type yet", () => {
    const devices = bindDevices(graph(), [set(1, rtl, [channel(3, "am")])]);
    const channels = bindChannels(graph(), devices);
    expect(channels.has("nfm")).toBe(false);
    expect(channels.get("am")?.id).toBe(3);
  });

  it("finds the radio behind a node in either direction", () => {
    const g: PatchGraph = {
      nodes: [
        ...graph().nodes,
        node("scan", { kind: "scanner" }),
        node("lost", { kind: "scanner" }),
      ],
      edges: [
        ...(graph().edges ?? []),
        { from: { node: "scan", port: "control" }, to: { node: "dev", port: "control" } },
      ],
    };
    expect(deviceNodeOf(g, "dev")).toBe("dev");
    expect(deviceNodeOf(g, "nfm")).toBe("dev");
    expect(deviceNodeOf(g, "scan")).toBe("dev");
    expect(deviceNodeOf(g, "lost")).toBeNull();
    expect(deviceNodeOf(g, "spk")).toBeNull();
  });

  it("walks the wires a sink consumes", () => {
    const g = graph();
    expect(channelNodesOf(g, "dev").map((wired) => wired.node.id)).toEqual(["nfm", "am"]);
    const devices = bindDevices(g, [set(1, rtl, [channel(9, "nfm")])]);
    const channels = bindChannels(g, devices);
    expect(inputsOf(g, "spk", "audio", devices, channels)).toEqual([
      { node: "nfm", deviceSet: 1, channel: channel(9, "nfm") },
    ]);
    expect(inputsOf(g, "spk", "audio", devices, new Map())).toEqual([]);
  });

  it("expands a trunk system into the traffic channels it is following", () => {
    const g: PatchGraph = {
      nodes: [
        node("dev", { kind: "device", data: { device: deviceRefOf(rtl) } }),
        node("dmr", { kind: "channel", data: { channel_type: "dmr" } }),
        node("trunk", { kind: "dmr_trunk", data: { protocol: "auto", retention_seconds: 300 } }),
        node("log", { kind: "decoder_log" }),
      ],
      edges: [
        { from: { node: "dev", port: "iq" }, to: { node: "dmr", port: "iq" } },
        { from: { node: "dmr", port: "events" }, to: { node: "trunk", port: "events" } },
        { from: { node: "trunk", port: "events" }, to: { node: "log", port: "events" } },
      ],
    };
    const control = channel(9, "dmr");
    const traffic = channel(10, "dmr");
    const devices = bindDevices(g, [set(1, rtl, [control, traffic])]);
    const channels = bindChannels(g, devices);
    const trunks = [
      {
        node: "trunk",
        carriers: 1,
        followers: [{ device_set: 1, channel: 10, slot: 2, freq_hz: 451_125_000 }],
        problems: [],
      },
    ];

    expect(inputsOf(g, "log", "events", devices, channels, trunks)).toEqual([
      { node: "trunk", deviceSet: 1, channel: traffic },
    ]);
    expect(inputsOf(g, "log", "events", devices, channels)).toEqual([]);
  });

  describe("multi-stream wires", () => {
    function lanes(): PatchGraph {
      return {
        nodes: [
          node("dev", { kind: "device", data: { device: deviceRefOf(rtl) } }),
          node("low", { kind: "channel", data: { channel_type: "nfm" } }),
          node("high", { kind: "channel", data: { channel_type: "nfm" } }),
          node("spk", { kind: "speaker" }),
        ],
        edges: [
          { from: { node: "dev", port: "iq" }, to: { node: "low", port: "iq" } },
          { from: { node: "dev", port: "iq3" }, to: { node: "high", port: "iq" } },
          { from: { node: "high", port: "audio" }, to: { node: "spk", port: "audio" } },
        ],
      };
    }

    it("reads the stream off the device end of the wire", () => {
      expect(iqSourceOf(lanes(), "high")).toEqual({ source: "dev", stream: 2 });
      expect(iqSourceOf(lanes(), "low")).toEqual({ source: "dev", stream: 0 });
      expect(iqSourceOf(lanes(), "dev")).toBeNull();
      expect(channelNodesOf(lanes(), "dev").map(({ node: n, stream }) => [n.id, stream])).toEqual([
        ["low", 0],
        ["high", 2],
      ]);
    });

    it("binds by type *and* stream so lanes of one radio cannot swap channels", () => {
      const live = set(1, rtl, [channel(5, "nfm", 2), channel(6, "nfm")]);
      const devices = bindDevices(lanes(), [live]);
      const channels = bindChannels(lanes(), devices);
      expect(channels.get("low")?.id).toBe(6);
      expect(channels.get("high")?.id).toBe(5);
      const partial = bindChannels(
        lanes(),
        bindDevices(lanes(), [set(1, rtl, [channel(6, "nfm")])]),
      );
      expect(partial.get("low")?.id).toBe(6);
      expect(partial.has("high")).toBe(false);
    });

    it("resolves the radio behind a node wired past stream 0", () => {
      const devices = bindDevices(lanes(), [set(1, rtl, [channel(5, "nfm", 2)])]);
      expect(deviceNodeOf(lanes(), "high")).toBe("dev");
      const channels = bindChannels(lanes(), devices);
      expect(inputsOf(lanes(), "spk", "audio", devices, channels)).toEqual([
        { node: "high", deviceSet: 1, channel: channel(5, "nfm", 2) },
      ]);
    });
  });
});

describe("refFromDeviceId", () => {
  it("splits on the first colon so a file key survives", () => {
    expect(refFromDeviceId("virtual:file:/data/rec/airband-2026")).toEqual({
      backend: "virtual",
      key: "file:/data/rec/airband-2026",
    });
    expect(refFromDeviceId("rtlsdr:00000001")).toEqual({
      backend: "rtlsdr",
      key: "00000001",
    });
  });

  it("round-trips a recording onto the device it names, and not onto the siggen", () => {
    const recording = refFromDeviceId("virtual:file:/data/rec/airband");
    expect(recording).not.toBeNull();
    if (recording === null) {
      throw new Error("parsed");
    }
    const playback: DeviceInfo = {
      driver: "virtual",
      key: "file:/data/rec/airband",
      label: "airband (recording)",
    };
    const siggen: DeviceInfo = { driver: "virtual", key: "siggen", label: "Signal Generator" };
    expect(refMatches(recording, playback)).toBe(true);
    expect(refMatches(recording, siggen)).toBe(false);
  });

  it("refuses a handle with no key rather than producing one that matches anything", () => {
    expect(refFromDeviceId("virtual")).toBeNull();
    expect(refFromDeviceId("virtual:")).toBeNull();
    expect(refFromDeviceId(":siggen")).toBeNull();
    expect(refFromDeviceId("")).toBeNull();
  });
});
