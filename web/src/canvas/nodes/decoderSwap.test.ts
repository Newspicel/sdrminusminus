import { describe, expect, it, vi } from "vitest";
import type {
  ChannelDescriptor,
  ChannelInfo,
  ChannelSettings,
  PatchCatalog,
  PatchGraph,
  PatchNode,
  PatchNodeOf,
  WorkspaceSnapshot,
} from "../../lib/types";
import type { GraphContext } from "../graph";
import {
  ANALOG_MODES,
  nextAnalogMode,
  retypeChannel,
  retypedSettings,
  swapDecoder,
} from "./decoderSwap";

const CATALOG: PatchCatalog = {
  nodes: [
    {
      kind: "device",
      name: "Device",
      category: "source",
      ports: [{ name: "iq", port_type: "iq", direction: "out", multi: true }],
    },
    {
      kind: "channel",
      name: "Channel",
      category: "channel",
      needs_channel_type: true,
      ports: [
        { name: "iq", port_type: "iq", direction: "in", multi: false },
        {
          name: "position",
          port_type: "position",
          direction: "in",
          multi: false,
          condition: "channel_needs_position",
        },
        { name: "baseband", port_type: "baseband", direction: "out", multi: true },
        {
          name: "audio",
          port_type: "audio",
          direction: "out",
          multi: true,
          condition: "channel_has_audio",
        },
        {
          name: "events",
          port_type: "events",
          direction: "out",
          multi: true,
          condition: "channel_is_decoder",
        },
      ],
    },
    {
      kind: "speaker",
      name: "Speaker",
      category: "output",
      ports: [{ name: "audio", port_type: "audio", direction: "in", multi: true }],
    },
    {
      kind: "decoder_log",
      name: "Decoder log",
      category: "output",
      ports: [{ name: "events", port_type: "events", direction: "in", multi: true }],
    },
  ],
};

const settings = (type: string, frequencyHz: number): ChannelSettings => ({
  frequency_hz: frequencyHz,
  params: { type, settings: {} } as ChannelSettings["params"],
  audio: {},
});

const descriptor = (
  type_id: string,
  extra: Partial<ChannelDescriptor> = {},
): ChannelDescriptor => ({
  type_id,
  name: type_id.toUpperCase(),
  bandwidth_hz: 12_500,
  input_rate_hz: 48_000,
  has_audio: true,
  defaults: settings(type_id, 100e6),
  ...extra,
});

const NFM = descriptor("nfm", { decoder_kind: "tone" });
const AM = descriptor("am");
const DMR = descriptor("dmr", { decoder_kind: "dv" });
const ADSB = descriptor("adsb", {
  has_audio: false,
  decoder_kind: "adsb",
  needs_position: true,
});

const context: GraphContext = {
  catalog: CATALOG,
  channelTypes: [NFM, AM, DMR, ADSB],
  facets: [],
};

function node(id: string, body: Partial<PatchNode> & Pick<PatchNode, "kind">): PatchNode {
  return { id, position: { x: 0, y: 0 }, ...body } as PatchNode;
}

function graph(): PatchGraph {
  return {
    nodes: [
      node("dev", { kind: "device", data: {} }),
      node("ch", { kind: "channel", data: { channel_type: "nfm", tuning_locked: true } }),
      node("spk", { kind: "speaker" }),
      node("log", { kind: "decoder_log" }),
    ],
    edges: [
      { from: { node: "dev", port: "iq" }, to: { node: "ch", port: "iq" } },
      { from: { node: "ch", port: "audio" }, to: { node: "spk", port: "audio" } },
      { from: { node: "ch", port: "events" }, to: { node: "log", port: "events" } },
    ],
  };
}

const channelNode = (patch: PatchGraph): PatchNodeOf<"channel"> => {
  const found = patch.nodes.find((candidate) => candidate.id === "ch");
  if (found?.kind !== "channel") {
    throw new Error("the graph carries a channel node");
  }
  return found;
};

describe("nextAnalogMode", () => {
  it("walks the ring in both directions", () => {
    expect(nextAnalogMode("nfm", 1)).toBe("wfm");
    expect(nextAnalogMode("nfm", -1)).toBe("ssb");
    expect(nextAnalogMode("ssb", 1)).toBe("nfm");
  });

  it("lands on the first mode from a decoder that is not analog", () => {
    expect(nextAnalogMode("adsb", 1)).toBe(ANALOG_MODES[0]);
    expect(nextAnalogMode("adsb", -1)).toBe(ANALOG_MODES[0]);
  });
});

describe("retypeChannel", () => {
  it("keeps what the node was set to apart from its type", () => {
    const retyped = retypeChannel(context, graph(), "ch", AM);
    expect(channelNode(retyped).data).toEqual({
      channel_type: "am",
      tuning_locked: true,
      record_calls: false,
    });
  });

  it("drops the wires the new decoder has no port for", () => {
    const retyped = retypeChannel(context, graph(), "ch", AM);
    expect(retyped.edges).toEqual([
      { from: { node: "dev", port: "iq" }, to: { node: "ch", port: "iq" } },
      { from: { node: "ch", port: "audio" }, to: { node: "spk", port: "audio" } },
    ]);
  });

  it("leaves a wire the new decoder still carries", () => {
    const retyped = retypeChannel(context, graph(), "ch", DMR);
    expect(retyped.edges).toHaveLength(3);
  });

  it("drops the audio wire a silent decoder cannot feed", () => {
    const retyped = retypeChannel(context, graph(), "ch", ADSB);
    expect(retyped.edges).toEqual([
      { from: { node: "dev", port: "iq" }, to: { node: "ch", port: "iq" } },
      { from: { node: "ch", port: "events" }, to: { node: "log", port: "events" } },
    ]);
  });

  it("stops recording calls on a decoder that has none", () => {
    const calling = retypeChannel(context, graph(), "ch", DMR);
    const started = { ...calling, nodes: calling.nodes };
    const back = retypeChannel(context, started, "ch", NFM);
    expect(channelNode(back).data).toMatchObject({ channel_type: "nfm", record_calls: false });
  });
});

describe("retypedSettings", () => {
  it("keeps where the decoder listens and how it is squelched", () => {
    const current: ChannelSettings = {
      ...settings("nfm", 145.5e6),
      squelch: { mode: "manual", level_db: -50 },
    };
    expect(retypedSettings(current, AM)).toEqual({
      frequency_hz: 145.5e6,
      params: { type: "am", settings: {} },
      audio: {},
      squelch: { mode: "manual", level_db: -50 },
    });
  });

  it("falls back to what the new decoder starts on", () => {
    expect(retypedSettings(null, AM)).toEqual(AM.defaults);
    expect(retypedSettings(null, { ...AM, defaults: null })).toBeNull();
  });
});

describe("swapDecoder", () => {
  const swap = (target: PatchNode, wanted: ChannelDescriptor, live: ChannelInfo | null) => {
    const applyEdit = vi.fn();
    const saveChannel = vi.fn();
    let snapshot: WorkspaceSnapshot = { version: 1, graph: graph() };
    const done = swapDecoder({
      context,
      node: target,
      descriptor: wanted,
      live: live === null ? null : { deviceSet: 3, channel: live },
      saved: settings("nfm", 145.5e6),
      applyEdit,
      saveChannel,
      edit: (change) => {
        snapshot = change(snapshot);
      },
    });
    return { done, applyEdit, saveChannel, snapshot };
  };

  it("patches the live channel and the node together", () => {
    const live: ChannelInfo = { id: 7, settings: settings("nfm", 145.5e6) };
    const result = swap(channelNode(graph()), AM, live);
    expect(result.done).toBe(true);
    expect(result.applyEdit).toHaveBeenCalledWith(3, 7, {
      frequency_hz: 145.5e6,
      params: { type: "am", settings: {} },
      audio: {},
    });
    expect(result.saveChannel).not.toHaveBeenCalled();
    expect(channelNode(result.snapshot.graph).data).toMatchObject({ channel_type: "am" });
  });

  it("remembers the swap on a node no radio carries", () => {
    const result = swap(channelNode(graph()), AM, null);
    expect(result.saveChannel).toHaveBeenCalledWith("ch", {
      frequency_hz: 145.5e6,
      params: { type: "am", settings: {} },
      audio: {},
    });
    expect(result.applyEdit).not.toHaveBeenCalled();
  });

  it("does nothing when the node already carries that decoder", () => {
    const result = swap(channelNode(graph()), NFM, null);
    expect(result.done).toBe(false);
    expect(result.saveChannel).not.toHaveBeenCalled();
    expect(result.snapshot.graph.edges).toHaveLength(3);
  });
});
