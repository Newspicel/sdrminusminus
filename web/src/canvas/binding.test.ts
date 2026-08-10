import { describe, expect, it } from "vitest";
import type { ChannelInfo, DeviceInfo, DeviceSet, PatchGraph, PatchNode } from "../lib/types";
import {
  bindChannels,
  bindDevices,
  channelNodesOf,
  deviceNodeOf,
  deviceRefOf,
  inputsOf,
  refMatches,
  unboundChannels,
} from "./binding";

function info(overrides: Partial<DeviceInfo>): DeviceInfo {
  return { driver: "rtlsdr", key: "0", label: "RTL-SDR", ...overrides };
}

function channel(id: number, type: string): ChannelInfo {
  return { id, settings: { offset_hz: 0, params: { type, settings: {} } as never } };
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
      tx_capable: false,
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

// The rules mirrored here are `DeviceRef::matches` and `apply_station`; these cases are the same
// ones `device_refs_match_by_serial_then_key_then_singleton` pins in Rust.
describe("device references", () => {
  it("prefers the serial, falls back to the key, and accepts a singleton backend", () => {
    const hardware = info({ serial: "00000001" });
    const bySerial = deviceRefOf(hardware);
    expect(bySerial).toEqual({ backend: "rtlsdr", serial: "00000001" });
    expect(refMatches(bySerial, hardware)).toBe(true);
    // Same radio, different USB port: the key moved, the serial did not.
    expect(refMatches(bySerial, info({ key: "3", serial: "00000001" }))).toBe(true);
    expect(refMatches(bySerial, info({ serial: "00000002" }))).toBe(false);

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

  // "Serial-less duplicate clones bind at most one node" (CANVAS §3).
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
    // The second NFM belongs to no node — the canvas says so rather than inventing a face.
    expect(unboundChannels(graph(), "dev", live, channels).map((c) => c.id)).toEqual([11]);
  });

  // Channel ids are per device set, so a second radio's channel 1 must not be hidden by the
  // first radio's channel 1 being on the canvas.
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

  // One resolver answers "which radio is this face about" for every kind of node, and the two
  // directions are not symmetric: a channel or a scope *takes* IQ from a radio, while a scanner
  // *drives* one — its wire leaves it.
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
    // A sink hangs off a channel, not off a radio: only the wire from a device answers.
    expect(deviceNodeOf(g, "spk")).toBeNull();
  });

  it("walks the wires a sink consumes", () => {
    const g = graph();
    expect(channelNodesOf(g, "dev").map((n) => n.id)).toEqual(["nfm", "am"]);
    const devices = bindDevices(g, [set(1, rtl, [channel(9, "nfm")])]);
    const channels = bindChannels(g, devices);
    expect(inputsOf(g, "spk", "audio", devices, channels)).toEqual([
      { node: "nfm", deviceSet: 1, channel: channel(9, "nfm") },
    ]);
    // A wire from a channel with no engine channel behind it contributes nothing to play.
    expect(inputsOf(g, "spk", "audio", devices, new Map())).toEqual([]);
  });
});
