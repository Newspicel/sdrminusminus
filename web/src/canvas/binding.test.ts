import { describe, expect, it } from "vitest";
import type { ChannelInfo, DeviceInfo, DeviceSet, PatchGraph, PatchNode } from "../lib/types";
import {
  audioSourcesOf,
  bindCarriers,
  bindChannels,
  bindDevices,
  channelNodesOf,
  controlledNodeOf,
  deviceNodeOf,
  deviceRefOf,
  eventSourcesOf,
  hasWire,
  inputsOf,
  iqLanesOf,
  iqSourceOf,
  ownersOf,
  refMatches,
  speakerInputsOf,
  tuningControllerOf,
} from "./binding";

function info(overrides: Partial<DeviceInfo>): DeviceInfo {
  return { driver: "rtlsdr", key: "0", label: "RTL-SDR", ...overrides };
}

function channel(id: number, type: string, stream = 0): ChannelInfo {
  return {
    id,
    stream,
    settings: { frequency_hz: 100_000_000, params: { type, settings: {} } as never },
  };
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

  it("binds array lanes and their channels alongside the source devices", () => {
    const source = set(1, rtl);
    const array = set(2, info({ driver: "array", key: "bench-pair" }), [channel(7, "nfm", 1)]);
    const g: PatchGraph = {
      nodes: [
        node("dev", { kind: "device", data: { device: deviceRefOf(rtl) } }),
        node("bench:pair", {
          kind: "array",
          data: { members: 2, coherence: "time_sync", shared_tuning: true },
        }),
        node("voice", { kind: "channel", data: { channel_type: "nfm" } }),
      ],
      edges: [{ from: { node: "bench:pair", port: "iq2" }, to: { node: "voice", port: "iq" } }],
    };
    const devices = bindDevices(g, [source, array]);
    expect(devices.get("dev")?.id).toBe(1);
    expect(devices.get("bench:pair")?.id).toBe(2);
    expect(bindChannels(g, devices).get("voice")?.id).toBe(7);
    expect(deviceNodeOf(g, "voice", ownersOf(bindCarriers(g, devices)))).toBe("bench:pair");
  });

  it("speakerInputsOf lists only the channels wired into a speaker", () => {
    const g = graph();
    const devices = bindDevices(g, [set(1, rtl, [channel(4, "nfm"), channel(5, "am")])]);
    const channels = bindChannels(g, devices);
    expect(speakerInputsOf(g, devices, channels)).toEqual([{ deviceSet: 1, channel: 4, fx: [] }]);

    const unwired: PatchGraph = {
      nodes: g.nodes.filter((n) => n.id !== "spk"),
      edges: g.edges?.filter((e) => e.to.node !== "spk"),
    };
    expect(speakerInputsOf(unwired, devices, channels)).toEqual([]);
  });

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
  });

  it("binds a node to the channel opened for it, whatever the stored order", () => {
    const g: PatchGraph = {
      nodes: [
        node("dev", { kind: "device", data: { device: deviceRefOf(rtl) } }),
        node("first", { kind: "channel", data: { channel_type: "adsb" } }),
        node("second", { kind: "channel", data: { channel_type: "adsb" } }),
      ],
      edges: [
        { from: { node: "dev", port: "iq" }, to: { node: "first", port: "iq" } },
        { from: { node: "dev", port: "iq" }, to: { node: "second", port: "iq" } },
      ],
    };
    const live = set(1, rtl, [
      { ...channel(4, "adsb"), node: "second" },
      { ...channel(5, "adsb"), node: "first" },
      channel(6, "adsb"),
    ]);
    const channels = bindChannels(g, bindDevices(g, [live]));
    expect(channels.get("first")?.id).toBe(5);
    expect(channels.get("second")?.id).toBe(4);

    const cut: PatchGraph = { ...g, nodes: g.nodes.filter((held) => held.id !== "first") };
    expect(bindChannels(cut, bindDevices(cut, [live])).get("second")?.id).toBe(4);
  });

  it("never hands a node a channel opened for another node", () => {
    const live = set(1, rtl, [{ ...channel(8, "nfm"), node: "someone-else" }]);
    const channels = bindChannels(graph(), bindDevices(graph(), [live]));
    expect(channels.has("nfm")).toBe(false);
  });

  it("never hands a node a channel a trunk system opened for itself", () => {
    const live = set(1, rtl, [channel(8, "dmr")]);
    const trunked: PatchGraph = {
      nodes: [...graph().nodes, node("talk", { kind: "channel", data: { channel_type: "dmr" } })],
      edges: [
        ...(graph().edges ?? []),
        { from: { node: "dev", port: "iq" }, to: { node: "talk", port: "iq" } },
      ],
    };
    const devices = bindDevices(trunked, [live]);
    expect(bindChannels(trunked, devices).get("talk")?.id).toBe(8);
    const trunk = {
      node: "sys",
      carriers: 1,
      followers: [{ device_set: 1, channel: 8, slot: 1, freq_hz: 451_000_000 }],
      problems: [],
    };
    expect(bindChannels(trunked, devices, [trunk]).has("talk")).toBe(false);
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
        node("orphan", { kind: "channel", data: { channel_type: "am" } }),
        node("stranded", { kind: "scanner" }),
      ],
      edges: [
        ...(graph().edges ?? []),
        { from: { node: "scan", port: "control" }, to: { node: "nfm", port: "control" } },
        { from: { node: "stranded", port: "control" }, to: { node: "orphan", port: "control" } },
      ],
    };
    expect(deviceNodeOf(g, "dev")).toBe("dev");
    expect(deviceNodeOf(g, "nfm")).toBe("dev");
    expect(deviceNodeOf(g, "scan")).toBe("dev");
    expect(deviceNodeOf(g, "lost")).toBeNull();
    expect(deviceNodeOf(g, "stranded")).toBeNull();
    expect(deviceNodeOf(g, "spk")).toBeNull();
    expect(controlledNodeOf(g, "scan")).toBe("nfm");
    expect(controlledNodeOf(g, "stranded")).toBe("orphan");
    expect(controlledNodeOf(g, "lost")).toBeNull();
    expect(controlledNodeOf(g, "nfm")).toBeNull();
  });

  it("walks the wires a sink consumes", () => {
    const g = graph();
    expect(channelNodesOf(g, "dev").map((wired) => wired.node.id)).toEqual(["nfm", "am"]);
    const devices = bindDevices(g, [set(1, rtl, [channel(9, "nfm")])]);
    const channels = bindChannels(g, devices);
    expect(inputsOf(g, "spk", "audio", devices, channels)).toEqual([
      { node: "nfm", deviceSet: 1, channel: channel(9, "nfm"), fx: [] },
    ]);
    expect(inputsOf(g, "spk", "audio", devices, new Map())).toEqual([]);
  });

  it("sees a wire on a port whose source carries nothing", () => {
    const g = graph();
    expect(hasWire(g, "spk", "audio")).toBe(true);
    expect(hasWire(g, "spk", "events")).toBe(false);
    expect(hasWire(g, "nfm", "iq")).toBe(true);
    expect(inputsOf(g, "spk", "audio", new Map(), new Map())).toEqual([]);
  });

  it("expands a trunk system into the traffic channels it is following", () => {
    const g: PatchGraph = {
      nodes: [
        node("dev", { kind: "device", data: { device: deviceRefOf(rtl) } }),
        node("dmr", { kind: "channel", data: { channel_type: "dmr" } }),
        node("trunk", { kind: "dmr_trunk", data: { protocol: "auto", record_calls: true } }),
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
        control: { device_set: 1, channel: 9, freq_hz: 451_012_500 },
        followers: [{ device_set: 1, channel: 10, slot: 2, freq_hz: 451_125_000 }],
        problems: [],
      },
    ];

    expect(inputsOf(g, "log", "events", devices, channels, trunks)).toEqual([
      { node: "trunk", deviceSet: 1, channel: control, fx: [] },
      { node: "trunk", deviceSet: 1, channel: traffic, fx: [] },
    ]);
    expect(inputsOf(g, "log", "events", devices, channels)).toEqual([]);
  });

  it("keeps a trunk system's control channel on the wire between calls", () => {
    const g: PatchGraph = {
      nodes: [
        node("dev", { kind: "device", data: { device: deviceRefOf(rtl) } }),
        node("trunk", { kind: "dmr_trunk", data: { protocol: "auto", record_calls: true } }),
        node("log", { kind: "decoder_log" }),
      ],
      edges: [
        { from: { node: "dev", port: "iq" }, to: { node: "trunk", port: "iq" } },
        { from: { node: "trunk", port: "events" }, to: { node: "log", port: "events" } },
      ],
    };
    const control = channel(9, "dmr");
    const devices = bindDevices(g, [set(1, rtl, [control])]);
    const channels = bindChannels(g, devices);
    const trunks = [
      {
        node: "trunk",
        carriers: 1,
        control: { device_set: 1, channel: 9, freq_hz: 451_012_500 },
        followers: [],
        problems: [],
      },
    ];

    expect(inputsOf(g, "log", "events", devices, channels, trunks)).toEqual([
      { node: "trunk", deviceSet: 1, channel: control, fx: [] },
    ]);
  });

  it("sees the decoder behind an event filter, however many are chained", () => {
    const g: PatchGraph = {
      nodes: [
        node("dev", { kind: "device", data: { device: deviceRefOf(rtl) } }),
        node("dmr", { kind: "channel", data: { channel_type: "dmr" } }),
        node("only-calls", { kind: "event_filter", data: { kinds: ["call"] } }),
        node("loud", { kind: "event_filter", data: { min_duration_ms: 1000 } }),
        node("log", { kind: "decoder_log" }),
      ],
      edges: [
        { from: { node: "dev", port: "iq" }, to: { node: "dmr", port: "iq" } },
        { from: { node: "dmr", port: "events" }, to: { node: "only-calls", port: "events" } },
        { from: { node: "only-calls", port: "events" }, to: { node: "loud", port: "events" } },
        { from: { node: "loud", port: "events" }, to: { node: "log", port: "events" } },
      ],
    };
    const dmr = channel(9, "dmr");
    const devices = bindDevices(g, [set(1, rtl, [dmr])]);
    const channels = bindChannels(g, devices);

    expect(inputsOf(g, "log", "events", devices, channels)).toEqual([
      { node: "dmr", deviceSet: 1, channel: dmr, fx: [] },
    ]);
    expect(eventSourcesOf(g, "log")).toEqual(["dmr"]);
  });

  it("names a decoder once when it reaches a sink filtered and direct", () => {
    const g: PatchGraph = {
      nodes: [
        node("dev", { kind: "device", data: { device: deviceRefOf(rtl) } }),
        node("dmr", { kind: "channel", data: { channel_type: "dmr" } }),
        node("only-calls", { kind: "event_filter", data: { kinds: ["call"] } }),
        node("log", { kind: "decoder_log" }),
      ],
      edges: [
        { from: { node: "dev", port: "iq" }, to: { node: "dmr", port: "iq" } },
        { from: { node: "dmr", port: "events" }, to: { node: "only-calls", port: "events" } },
        { from: { node: "only-calls", port: "events" }, to: { node: "log", port: "events" } },
        { from: { node: "dmr", port: "events" }, to: { node: "log", port: "events" } },
      ],
    };

    expect(eventSourcesOf(g, "log")).toEqual(["dmr"]);
  });

  it("follows audio through a chain of audio FX nodes, channel side first", () => {
    const g: PatchGraph = {
      nodes: [
        node("dev", { kind: "device", data: { device: deviceRefOf(rtl) } }),
        node("nfm", { kind: "channel", data: { channel_type: "nfm" } }),
        node("near", { kind: "audio_fx", data: { settings: {} } }),
        node("far", { kind: "audio_fx", data: { settings: {} } }),
        node("spk", { kind: "speaker" }),
      ],
      edges: [
        { from: { node: "dev", port: "iq" }, to: { node: "nfm", port: "iq" } },
        { from: { node: "nfm", port: "audio" }, to: { node: "near", port: "audio" } },
        { from: { node: "near", port: "audio" }, to: { node: "far", port: "audio" } },
        { from: { node: "far", port: "audio" }, to: { node: "spk", port: "audio" } },
        { from: { node: "nfm", port: "audio" }, to: { node: "spk", port: "audio" } },
      ],
    };
    const nfm = channel(3, "nfm");
    const devices = bindDevices(g, [set(1, rtl, [nfm])]);
    const channels = bindChannels(g, devices);

    expect(inputsOf(g, "spk", "audio", devices, channels)).toEqual([
      { node: "nfm", deviceSet: 1, channel: nfm, fx: ["near", "far"] },
      { node: "nfm", deviceSet: 1, channel: nfm, fx: [] },
    ]);
    expect(speakerInputsOf(g, devices, channels)).toEqual([
      { deviceSet: 1, channel: 3, fx: ["near", "far"] },
      { deviceSet: 1, channel: 3, fx: [] },
    ]);
    expect(audioSourcesOf(g, "far")).toEqual([{ node: "nfm", fx: ["near"] }]);
  });

  it("gives up on an audio FX chain deeper than the cap", () => {
    const chain = Array.from({ length: 40 }, (_, at) => `fx${at}`);
    const g: PatchGraph = {
      nodes: [
        node("nfm", { kind: "channel", data: { channel_type: "nfm" } }),
        ...chain.map((id) => node(id, { kind: "audio_fx", data: { settings: {} } })),
        node("spk", { kind: "speaker" }),
      ],
      edges: [
        { from: { node: "nfm", port: "audio" }, to: { node: "fx0", port: "audio" } },
        ...chain.slice(1).map((id, at) => ({
          from: { node: chain[at] ?? "", port: "audio" },
          to: { node: id, port: "audio" },
        })),
        { from: { node: chain.at(-1) ?? "", port: "audio" }, to: { node: "spk", port: "audio" } },
      ],
    };
    expect(audioSourcesOf(g, "spk")).toEqual([]);
  });

  it("does not walk audio wires looking for filters", () => {
    const g: PatchGraph = {
      nodes: [
        node("dev", { kind: "device", data: { device: deviceRefOf(rtl) } }),
        node("nfm", { kind: "channel", data: { channel_type: "nfm" } }),
        node("spk", { kind: "speaker" }),
      ],
      edges: [
        { from: { node: "dev", port: "iq" }, to: { node: "nfm", port: "iq" } },
        { from: { node: "nfm", port: "audio" }, to: { node: "spk", port: "audio" } },
      ],
    };
    const nfm = channel(3, "nfm");
    const devices = bindDevices(g, [set(1, rtl, [nfm])]);
    const channels = bindChannels(g, devices);

    expect(inputsOf(g, "spk", "audio", devices, channels)).toEqual([
      { node: "nfm", deviceSet: 1, channel: nfm, fx: [] },
    ]);
  });

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

  describe("multi-stream wires", () => {
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

    it("lists every radio wired into a channel, in wire order", () => {
      const twin: PatchGraph = {
        nodes: [
          ...lanes().nodes,
          node("dev2", { kind: "device", data: { device: deviceRefOf(other) } }),
        ],
        edges: [
          ...(lanes().edges ?? []),
          { from: { node: "dev2", port: "iq2" }, to: { node: "high", port: "iq" } },
        ],
      };
      expect(iqLanesOf(twin, "high")).toEqual([
        { source: "dev", stream: 2 },
        { source: "dev2", stream: 1 },
      ]);
      expect(iqSourceOf(twin, "high")).toEqual({ source: "dev", stream: 2 });
      expect(channelNodesOf(twin, "dev2").map(({ node: n, stream }) => [n.id, stream])).toEqual([
        ["high", 1],
      ]);
    });

    it("finds a channel on whichever wired radio carries it", () => {
      const twin: PatchGraph = {
        nodes: [
          ...lanes().nodes,
          node("dev2", { kind: "device", data: { device: deviceRefOf(other) } }),
        ],
        edges: [
          ...(lanes().edges ?? []),
          { from: { node: "dev2", port: "iq2" }, to: { node: "high", port: "iq" } },
        ],
      };
      const carried = { ...channel(9, "nfm", 1), node: "high" };
      const devices = bindDevices(twin, [
        set(1, rtl, [channel(6, "nfm")]),
        set(2, other, [carried]),
      ]);
      const carriers = bindCarriers(twin, devices);
      expect(carriers.get("high")).toEqual({ owner: "dev2", channel: carried });
      expect(carriers.get("low")).toEqual({ owner: "dev", channel: channel(6, "nfm") });
      const owners = ownersOf(carriers);
      expect(deviceNodeOf(twin, "high", owners)).toBe("dev2");
      expect(deviceNodeOf(twin, "high")).toBe("dev");
      const channels = bindChannels(twin, devices);
      expect(inputsOf(twin, "spk", "audio", devices, channels, [], owners)).toEqual([
        { node: "high", deviceSet: 2, channel: carried, fx: [] },
      ]);
      expect(speakerInputsOf(twin, devices, channels, [], owners)).toEqual([
        { deviceSet: 2, channel: 9, fx: [] },
      ]);
    });

    it("hands a nameless decoder only to the channel whose first wire is that radio", () => {
      const twin: PatchGraph = {
        nodes: [
          ...lanes().nodes,
          node("dev2", { kind: "device", data: { device: deviceRefOf(other) } }),
        ],
        edges: [
          ...(lanes().edges ?? []),
          { from: { node: "dev2", port: "iq" }, to: { node: "high", port: "iq" } },
        ],
      };
      const devices = bindDevices(twin, [set(1, rtl, []), set(2, other, [channel(4, "nfm")])]);
      expect(bindCarriers(twin, devices).has("high")).toBe(false);
    });

    it("finds the active stream when two lanes share a radio", () => {
      const patch = lanes();
      patch.edges = [
        ...(patch.edges ?? []),
        { from: { node: "dev", port: "iq2" }, to: { node: "high", port: "iq" } },
      ];
      const carried = { ...channel(9, "nfm", 1), node: "high" };
      const devices = bindDevices(patch, [set(1, rtl, [carried])]);
      expect(bindCarriers(patch, devices).get("high")).toEqual({ owner: "dev", channel: carried });
    });

    it("prefers a named decoder on another radio over an unnamed match", () => {
      const patch = lanes();
      patch.nodes.push(node("dev2", { kind: "device", data: { device: deviceRefOf(other) } }));
      patch.edges = [
        ...(patch.edges ?? []),
        { from: { node: "dev2", port: "iq" }, to: { node: "high", port: "iq" } },
      ];
      const carried = { ...channel(9, "nfm"), node: "high" };
      const devices = bindDevices(patch, [
        set(1, rtl, [channel(8, "nfm", 2)]),
        set(2, other, [carried]),
      ]);
      expect(bindCarriers(patch, devices).get("high")).toEqual({ owner: "dev2", channel: carried });
    });

    it("resolves the radio behind a node wired past stream 0", () => {
      const devices = bindDevices(lanes(), [set(1, rtl, [channel(5, "nfm", 2)])]);
      expect(deviceNodeOf(lanes(), "high")).toBe("dev");
      const channels = bindChannels(lanes(), devices);
      expect(inputsOf(lanes(), "spk", "audio", devices, channels)).toEqual([
        { node: "high", deviceSet: 1, channel: channel(5, "nfm", 2), fx: [] },
      ]);
    });
  });
});

function controlledBy(from: PatchNode): PatchGraph {
  return {
    nodes: [
      from,
      { id: "voice", kind: "channel", data: { channel_type: "nfm" }, position: { x: 0, y: 0 } },
    ],
    edges: [{ from: { node: from.id, port: "control" }, to: { node: "voice", port: "control" } }],
  };
}

describe("tuningControllerOf", () => {
  it("names the scanner or satellite that tunes a decoder", () => {
    expect(
      tuningControllerOf(
        controlledBy({ id: "sat", kind: "satellite", data: {}, position: { x: 0, y: 0 } }),
        "voice",
      ),
    ).toBe("Satellite");
    expect(
      tuningControllerOf(
        controlledBy({ id: "scan", kind: "scanner", data: {}, label: "Airband", position: { x: 0, y: 0 } }),
        "voice",
      ),
    ).toBe("Airband");
  });

  it("leaves a hunted decoder free, since a hunt only listens", () => {
    expect(
      tuningControllerOf(
        controlledBy({
          id: "hunt",
          kind: "hunt",
          data: { clicks: true },
          position: { x: 0, y: 0 },
        }),
        "voice",
      ),
    ).toBeNull();
  });
});

describe("beam wires", () => {
  const kraken = info({ serial: "K" });

  function beam(): PatchGraph {
    return {
      nodes: [
        node("dev", { kind: "device", data: { device: deviceRefOf(kraken) } }),
        node("comb", { kind: "combiner", data: {} }),
        node("scope", { kind: "scope" }),
        node("nfm", { kind: "channel", data: { channel_type: "nfm" } }),
      ],
      edges: [
        { from: { node: "dev", port: "iq2" }, to: { node: "comb", port: "iq" } },
        { from: { node: "dev", port: "iq3" }, to: { node: "comb", port: "iq2" } },
        { from: { node: "comb", port: "beam" }, to: { node: "scope", port: "iq" } },
        { from: { node: "comb", port: "beam" }, to: { node: "nfm", port: "iq" } },
      ],
    };
  }

  function open(channels: ChannelInfo[] = []): DeviceSet {
    const live = set(1, kraken, channels);
    return { ...live, capabilities: { ...live.capabilities, rx_streams: 5 } };
  }

  it("follow the combiner back to its radio, one lane past the last", () => {
    const devices = bindDevices(beam(), [open()]);
    expect(iqSourceOf(beam(), "scope", devices)).toEqual({
      source: "dev",
      stream: 5,
      beam: { node: "comb", port: "beam", tunes: 1 },
    });
    expect(deviceNodeOf(beam(), "scope")).toBe("dev");
  });

  it("bind a channel listening on the beam", () => {
    const devices = bindDevices(beam(), [open([channel(7, "nfm", 5), channel(8, "nfm")])]);
    expect(bindChannels(beam(), devices).get("nfm")?.id).toBe(7);
  });

  it("match nothing while the radio is closed", () => {
    expect(iqSourceOf(beam(), "scope")?.stream).toBe(-1);
  });

  it("follow a stitch's wide output and tune the wide lane itself", () => {
    const graph: PatchGraph = {
      nodes: [
        node("dev", { kind: "device", data: { device: deviceRefOf(kraken) } }),
        node("wide", { kind: "stitch", data: {} }),
        node("scope", { kind: "scope" }),
      ],
      edges: [
        { from: { node: "dev", port: "iq" }, to: { node: "wide", port: "iq" } },
        { from: { node: "dev", port: "iq2" }, to: { node: "wide", port: "iq2" } },
        { from: { node: "wide", port: "wide" }, to: { node: "scope", port: "iq" } },
      ],
    };
    const devices = bindDevices(graph, [open()]);
    expect(iqSourceOf(graph, "scope", devices)).toEqual({
      source: "dev",
      stream: 5,
      beam: { node: "wide", port: "wide", tunes: 5 },
    });
  });
});
