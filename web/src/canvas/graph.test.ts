import { describe, expect, it } from "vitest";
import type {
  ChannelDescriptor,
  DeviceSet,
  PatchCatalog,
  PatchGraph,
  PatchNode,
  PortCondition,
  PortSpec,
  WorkspaceSnapshot,
} from "../lib/types";
import {
  addEdge,
  connectionRefusal,
  edgeKey,
  edgeWarning,
  type GraphContext,
  isPinned,
  migrateSnapshot,
  moveSlot,
  NODE_MIN_SIZE,
  newNodeId,
  nodeMinSize,
  PORT_STEP_PX,
  PORT_TOP_PX,
  pin,
  placeSlot,
  portLabel,
  portStream,
  portsOf,
  pruneRack,
  RACK_COLS,
  removeNode,
  resizeSlot,
  sameGraph,
  streamLabel,
  streamPort,
  unpin,
} from "./graph";

const CATALOG: PatchCatalog = {
  nodes: [
    {
      kind: "device",
      name: "Device",
      category: "source",
      ports: [
        { name: "control", port_type: "control", direction: "in", multi: false },
        {
          name: "tx",
          port_type: "tx",
          direction: "in",
          multi: false,
          condition: "device_is_tx_capable",
          note: "reserved: transmit is not built ()",
          repeat: "per_tx_stream",
        },
        { name: "iq", port_type: "iq", direction: "out", multi: true, repeat: "per_rx_stream" },
      ],
    },
    {
      kind: "channel",
      name: "Channel",
      category: "channel",
      needs_channel_type: true,
      ports: [
        { name: "iq", port_type: "iq", direction: "in", multi: false },
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
        {
          name: "video",
          port_type: "video",
          direction: "out",
          multi: true,
          condition: "channel_has_video",
        },
      ],
    },
    {
      kind: "scope",
      name: "Scope",
      category: "display",
      ports: [{ name: "iq", port_type: "iq", direction: "in", multi: false }],
    },
    {
      kind: "speaker",
      name: "Speaker",
      category: "sink",
      ports: [{ name: "audio", port_type: "audio", direction: "in", multi: true }],
    },
    {
      kind: "scanner",
      name: "Scanner",
      category: "feature",
      ports: [{ name: "control", port_type: "control", direction: "out", multi: false }],
    },
  ],
};

const TYPES: ChannelDescriptor[] = [
  {
    type_id: "nfm",
    name: "NFM",
    bandwidth_hz: 12_500,
    input_rate_hz: 48_000,
    has_audio: true,
    exact_rate_only: false,
  },
  {
    type_id: "adsb",
    name: "ADS-B",
    bandwidth_hz: 2_000_000,
    input_rate_hz: 2_000_000,
    has_audio: false,
    decoder_kind: "adsb",
    exact_rate_only: false,
    native_rate_max_hz: 4_000_000,
  },
  {
    type_id: "atv",
    name: "ATV",
    bandwidth_hz: 6_000_000,
    input_rate_hz: 6_000_000,
    has_audio: false,
    has_video: true,
    exact_rate_only: false,
  },
];

const context: GraphContext = { catalog: CATALOG, channelTypes: TYPES };

function node(id: string, body: Partial<PatchNode> & Pick<PatchNode, "kind">): PatchNode {
  return { id, position: { x: 0, y: 0 }, ...body } as PatchNode;
}

function workspace(): PatchGraph {
  return {
    nodes: [
      node("dev", { kind: "device", data: {} }),
      node("scope", { kind: "scope" }),
      node("nfm", { kind: "channel", data: { channel_type: "nfm" } }),
      node("spk", { kind: "speaker" }),
    ],
    edges: [
      { from: { node: "dev", port: "iq" }, to: { node: "scope", port: "iq" } },
      { from: { node: "dev", port: "iq" }, to: { node: "nfm", port: "iq" } },
    ],
  };
}

const port = (n: string, p: string) => ({ node: n, port: p });

/** A device node bound to an attached radio, carrying the three things the rules read off one:
 * the rate the live rate warning measures a channel against, whether there is a transmitter to
 * draw an input for, and how many receive streams there are to draw outputs for. */
const bound = (node: string, radio: { rate?: number; tx?: boolean; rx?: number }) =>
  new Map([
    [
      node,
      {
        settings: { sample_rate: radio.rate },
        capabilities: { duplex: radio.tx === true ? "half" : "rx_only", rx_streams: radio.rx },
      } as DeviceSet,
    ],
  ]);

const deviceNode = (graph: PatchGraph): PatchNode => {
  const dev = graph.nodes[0];
  if (dev === undefined) {
    throw new Error("the workspace has a device node");
  }
  return dev;
};

describe("portLabel", () => {
  it("numbers the first stream only when there is a second to tell it from", () => {
    const one = [{ name: "iq", port_type: "iq", direction: "out", multi: true }] as PortSpec[];
    expect(portLabel("iq", one)).toBe("iq");

    const two = [
      { name: "iq", port_type: "iq", direction: "out", multi: true },
      { name: "iq2", port_type: "iq", direction: "out", multi: true },
    ] as PortSpec[];
    expect(portLabel("iq", two)).toBe("iq1");
    expect(portLabel("iq2", two)).toBe("iq2");

    expect(portLabel("control", two)).toBe("control");
  });

  it("leaves the wire name alone", () => {
    expect(streamPort("iq", 0)).toBe("iq");
    expect(streamLabel("iq", 0, 1)).toBe("iq");
    expect(streamLabel("iq", 0, 4)).toBe("iq1");
    expect(streamLabel("iq", 3, 4)).toBe("iq4");
  });
});

describe("ports", () => {
  it("resolves a channel's conditional outputs against its type", () => {
    const graph = workspace();
    const nfm = graph.nodes[2];
    const adsb = node("adsb", { kind: "channel", data: { channel_type: "adsb" } });
    const atv = node("atv", { kind: "channel", data: { channel_type: "atv" } });
    expect(nfm && portsOf(context, graph, nfm).map((p) => p.name)).toEqual(["iq", "audio"]);
    expect(portsOf(context, graph, adsb).map((p) => p.name)).toEqual(["iq", "events"]);
    expect(portsOf(context, graph, atv).map((p) => p.name)).toEqual(["iq", "video"]);
  });

  /** A condition this build cannot answer is not a port: a handle drawn on a guess accepts a wire
   * the server then refuses (mirrors `PortSpec::applies_to`). */
  it("leaves off a port whose condition it does not know", () => {
    const graph = workspace();
    const nfm = graph.nodes[2];
    const catalog: PatchCatalog = {
      nodes: CATALOG.nodes.map((entry) =>
        entry.kind === "channel"
          ? {
              ...entry,
              ports: entry.ports.map((spec) =>
                spec.name === "audio"
                  ? { ...spec, condition: "channel_is_telepathic" as PortCondition }
                  : spec,
              ),
            }
          : entry,
      ),
    };
    expect(nfm && portsOf({ ...context, catalog }, graph, nfm).map((p) => p.name)).toEqual(["iq"]);
  });

  it("gives an unknown channel type only its input", () => {
    const graph = workspace();
    const ghost = node("x", { kind: "channel", data: { channel_type: "wefax" } });
    expect(portsOf(context, graph, ghost).map((p) => p.name)).toEqual(["iq"]);
  });

  /** An RTL-SDR has no transmitter, so the node standing for one has no socket to key. The port
   * follows the radio, not the node kind — which is why it is answered from the binding. */
  it("draws a transmit input only on a radio that has one", () => {
    const graph = workspace();
    const dev = deviceNode(graph);
    const receiver = { ...context, bound: bound("dev", { tx: false }) };
    expect(portsOf(receiver, graph, dev).map((p) => p.name)).toEqual(["control", "iq"]);

    const transceiver = { ...context, bound: bound("dev", { tx: true }) };
    expect(portsOf(transceiver, graph, dev).map((p) => p.name)).toEqual(["control", "tx", "iq"]);

    // Nothing is attached: a radio out of reach keeps the ports the patch can vouch for, and the
    // one it cannot is left off rather than promised.
    expect(portsOf(context, graph, dev).map((p) => p.name)).toEqual(["control", "iq"]);
  });

  it("expands the IQ family to one output per receive stream of the attached radio", () => {
    const graph = workspace();
    const dev = deviceNode(graph);
    const four = { ...context, bound: bound("dev", { rx: 4 }) };
    const ports = portsOf(four, graph, dev);
    expect(ports.map((p) => p.name)).toEqual(["control", "iq", "iq2", "iq3", "iq4"]);
    expect(ports.every((p) => (p.repeat ?? "once") === "once")).toBe(true);
    expect(connectionRefusal(four, graph, port("dev", "iq3"), port("spk", "audio"))).toMatch(
      /iq cannot feed a audio input/,
    );
    const withScope = { ...graph, nodes: [...graph.nodes, node("scope2", { kind: "scope" })] };
    expect(connectionRefusal(four, withScope, port("dev", "iq3"), port("scope2", "iq"))).toBeNull();
    expect(portsOf({ ...context, bound: bound("dev", { rx: 99 }) }, graph, dev)).toHaveLength(17);
  });

  /** The critical unbound case: a workspace laid out against a four-stream radio keeps its wires
   * while the radio is away — a port with no handle is an edge React Flow will not draw. */
  it("keeps the streams stored wires name while the radio is absent", () => {
    const graph = addEdge(workspace(), {
      from: { node: "dev", port: "iq3" },
      to: { node: "scope", port: "iq" },
    });
    const dev = deviceNode(graph);
    expect(portsOf(context, graph, dev).map((p) => p.name)).toEqual(["control", "iq", "iq3"]);
    const two = { ...context, bound: bound("dev", { rx: 2 }) };
    expect(portsOf(two, graph, dev).map((p) => p.name)).toEqual(["control", "iq", "iq2", "iq3"]);
  });

  it("numbers stream ports from two and answers only canonical spellings", () => {
    expect(streamPort("iq", 0)).toBe("iq");
    expect(streamPort("iq", 2)).toBe("iq3");
    expect(portStream("iq", "iq")).toBe(0);
    expect(portStream("iq", "iq16")).toBe(15);
    for (const name of ["iq1", "iq0", "iq02", "iq17", "iqx", "tx2"]) {
      expect(portStream("iq", name)).toBeNull();
    }
  });

  /** Ports sit at `PORT_TOP_PX + PORT_STEP_PX × index`: five outputs run past the device kind's
   * 120px floor, so the floor follows the port count or the resizer clips the lowest handles. */
  it("grows the resize floor with the port count", () => {
    const graph = workspace();
    const dev = deviceNode(graph);
    const five = portsOf({ ...context, bound: bound("dev", { rx: 5 }) }, graph, dev);
    expect(nodeMinSize("device", five)).toEqual({
      w: NODE_MIN_SIZE.device.w,
      h: PORT_TOP_PX + PORT_STEP_PX * 5,
    });
    expect(nodeMinSize("device", five).h).toBeGreaterThan(NODE_MIN_SIZE.device.h);
    expect(nodeMinSize("device", portsOf(context, graph, dev))).toEqual(NODE_MIN_SIZE.device);
  });
});

describe("connectionRefusal", () => {
  it("accepts the wires the model is for", () => {
    const graph = workspace();
    expect(
      connectionRefusal(context, graph, port("nfm", "audio"), port("spk", "audio")),
    ).toBeNull();
    const withScope = { ...graph, nodes: [...graph.nodes, node("scope2", { kind: "scope" })] };
    expect(
      connectionRefusal(context, withScope, port("dev", "iq"), port("scope2", "iq")),
    ).toBeNull();
  });

  it("names the reason for every refusal", () => {
    const graph = workspace();
    expect(connectionRefusal(context, graph, port("dev", "iq"), port("dev", "iq"))).toMatch(
      /itself/,
    );
    expect(connectionRefusal(context, graph, port("dev", "iq"), port("spk", "audio"))).toMatch(
      /iq cannot feed a audio input/,
    );
    expect(connectionRefusal(context, graph, port("scope", "iq"), port("nfm", "iq"))).toMatch(
      /output to an input/,
    );
    expect(connectionRefusal(context, graph, port("dev", "iq"), port("nfm", "tap"))).toMatch(
      /does not exist/,
    );
    expect(connectionRefusal(context, graph, port("dev", "iq"), port("nfm", "iq"))).toMatch(
      /already wired/,
    );
  });

  it("refuses a second device on a channel and names why", () => {
    const graph = {
      ...workspace(),
      nodes: [...workspace().nodes, node("dev2", { kind: "device", data: {} })],
    };
    expect(connectionRefusal(context, graph, port("dev2", "iq"), port("nfm", "iq"))).toMatch(
      /coherent array/,
    );
  });

  it("wires a scanner into the radio it drives, and only one", () => {
    const graph = {
      ...workspace(),
      nodes: [
        ...workspace().nodes,
        node("scan", { kind: "scanner" }),
        node("dev2", { kind: "device", data: {} }),
      ],
    };
    expect(connectionRefusal(context, graph, port("scan", "control"), port("dev", "control"))) //
      .toBeNull();
    // Ownership does not fan out: the engine runs one sweep per radio, either way round.
    const driving = addEdge(graph, {
      from: { node: "scan", port: "control" },
      to: { node: "dev", port: "control" },
    });
    expect(
      connectionRefusal(context, driving, port("scan", "control"), port("dev2", "control")),
    ).toMatch(/one node at a time/);
    const second = { ...driving, nodes: [...driving.nodes, node("scan2", { kind: "scanner" })] };
    expect(
      connectionRefusal(context, second, port("scan2", "control"), port("dev", "control")),
    ).toMatch(/takes one wire/);
  });

  it("refuses everything at the reserved transmit input, with the reason", () => {
    const graph = {
      ...workspace(),
      nodes: [...workspace().nodes, node("dev2", { kind: "device", data: {} })],
    };
    const transceiver = { ...context, bound: bound("dev2", { tx: true }) };
    expect(connectionRefusal(transceiver, graph, port("dev", "iq"), port("dev2", "tx"))).toMatch(
      /transmit is not built/,
    );
    expect(connectionRefusal(context, graph, port("dev", "iq"), port("dev2", "tx"))).toMatch(
      /does not exist/,
    );
  });

  /// : the rate rule is a fault *on* the wire, not a refusal of it — the rate is one
  /// setting away, and the face at the end of the wire offers that setting.
  it("allows a wideband channel on a rate outside its range and marks the wire instead", () => {
    const graph = {
      ...workspace(),
      nodes: [
        ...workspace().nodes,
        node("adsb", { kind: "channel", data: { channel_type: "adsb" } }),
      ],
    };
    const wrong = { ...context, bound: bound("dev", { rate: 10_000_000 }) };
    expect(connectionRefusal(wrong, graph, port("dev", "iq"), port("adsb", "iq"))).toBeNull();

    expect(edgeWarning(wrong, graph, port("dev", "iq"), port("adsb", "iq"))).toBe(
      "needs 2.000–4.000 MHz",
    );

    const right = { ...context, bound: bound("dev", { rate: 2_048_000 }) };
    expect(edgeWarning(right, graph, port("dev", "iq"), port("adsb", "iq"))).toBeNull();
    expect(edgeWarning(context, graph, port("dev", "iq"), port("adsb", "iq"))).toBeNull();
    expect(edgeWarning(wrong, graph, port("dev", "iq"), port("nfm", "iq"))).toBeNull();
  });
});

describe("editing", () => {
  it("removing a node takes its wires with it", () => {
    const graph = removeNode(workspace(), "nfm");
    expect(graph.nodes.map((n) => n.id)).toEqual(["dev", "scope", "spk"]);
    expect(graph.edges?.map(edgeKey)).toEqual(["dev.iq->scope.iq"]);
  });

  it("ids are unique per node", () => {
    expect(newNodeId("scope")).not.toEqual(newNodeId("scope"));
    expect(newNodeId("scope").startsWith("scope:")).toBe(true);
  });

  it("turns a stored scanner's IQ wire into the control wire that drives the radio", () => {
    const scanning = (edges: PatchGraph["edges"]): WorkspaceSnapshot => ({
      version: 1,
      graph: {
        nodes: [
          ...workspace().nodes,
          node("scan", { kind: "scanner" }),
          node("dev2", { kind: "device", data: {} }),
        ],
        edges,
      },
    });
    const stored = scanning([
      { from: { node: "dev", port: "iq" }, to: { node: "scope", port: "iq" } },
      { from: { node: "dev", port: "iq" }, to: { node: "scan", port: "iq" } },
    ]);
    expect(migrateSnapshot(stored).graph.edges?.map(edgeKey)).toEqual([
      "dev.iq->scope.iq",
      "scan.control->dev.control",
    ]);
    const migrated = migrateSnapshot(stored);
    expect(migrateSnapshot(migrated)).toBe(migrated);

    // Ownership is exclusive at both ends, so a sweep that fanned across two radios keeps the
    // first: the alternative is a snapshot the server refuses on the next write.
    const fanned = scanning([
      { from: { node: "dev", port: "iq" }, to: { node: "scan", port: "iq" } },
      { from: { node: "dev2", port: "iq" }, to: { node: "scan", port: "iq" } },
    ]);
    expect(migrateSnapshot(fanned).graph.edges?.map(edgeKey)).toEqual([
      "scan.control->dev.control",
    ]);
  });

  it("compares graphs structurally so an echo of our own write is not re-applied", () => {
    expect(sameGraph(workspace(), workspace())).toBe(true);
    expect(
      sameGraph(
        workspace(),
        addEdge(workspace(), {
          from: { node: "nfm", port: "audio" },
          to: { node: "spk", port: "audio" },
        }),
      ),
    ).toBe(false);
  });
});

describe("the rack", () => {
  it("pins into the first free cell and unpins", () => {
    let rack = pin({ slots: [] }, "scope");
    expect(rack.slots).toEqual([{ node: "scope", x: 0, y: 0, w: 6, h: 4 }]);
    rack = pin(rack, "nfm");
    expect(rack.slots?.[1]).toEqual({ node: "nfm", x: 6, y: 0, w: 6, h: 4 });
    // A third face wraps to the next row rather than overlapping.
    rack = pin(rack, "spk");
    expect(rack.slots?.[2]).toEqual({ node: "spk", x: 0, y: 4, w: 6, h: 4 });

    expect(isPinned(rack, "nfm")).toBe(true);
    rack = unpin(rack, "nfm");
    expect(isPinned(rack, "nfm")).toBe(false);
    expect(pin(rack, "scope")).toBe(rack);
  });

  it("refuses a placement that overlaps or leaves the grid", () => {
    const rack = pin(pin({ slots: [] }, "a"), "b");
    expect(placeSlot(rack, "b", { x: 0, y: 0, w: 6, h: 4 })).toBe(rack);
    expect(placeSlot(rack, "b", { x: RACK_COLS - 2, y: 0, w: 6, h: 4 })).toBe(rack);
    expect(placeSlot(rack, "b", { x: 0, y: 4, w: 6, h: 4 }).slots?.[1]).toEqual({
      node: "b",
      x: 0,
      y: 4,
      w: 6,
      h: 4,
    });
    expect(placeSlot(rack, "a", { x: 0, y: 0, w: 6, h: 8 }).slots?.[0]?.h).toBe(8);
  });

  it("trades places when a face is dropped on another", () => {
    const rack = placeSlot(pin(pin({ slots: [] }, "a"), "b"), "b", { x: 6, y: 0, w: 6, h: 8 });
    const swapped = moveSlot(rack, "a", { x: 6, y: 0 });
    expect(swapped.slots).toEqual([
      { node: "a", x: 6, y: 0, w: 6, h: 8 },
      { node: "b", x: 0, y: 0, w: 6, h: 4 },
    ]);
    expect(moveSlot(rack, "a", { x: 0, y: 4 }).slots?.[0]).toEqual({
      node: "a",
      x: 0,
      y: 4,
      w: 6,
      h: 4,
    });
    // Off the grid, and onto two faces at once, are both refused rather than clamped.
    expect(moveSlot(rack, "a", { x: RACK_COLS - 1, y: 0 })).toBe(rack);
    const three = placeSlot(pin(rack, "c"), "c", { x: 6, y: 4, w: 6, h: 4 });
    expect(moveSlot(three, "a", { x: 5, y: 2 })).toBe(three);
  });

  it("moves the boundary between two faces, one growing as the other shrinks", () => {
    const rack = pin(pin({ slots: [] }, "a"), "b");
    const wider = resizeSlot(rack, "a", "e", 2);
    expect(wider.slots).toEqual([
      { node: "a", x: 0, y: 0, w: 8, h: 4 },
      { node: "b", x: 8, y: 0, w: 4, h: 4 },
    ]);
    expect(resizeSlot(wider, "b", "w", -2)).toEqual(rack);
    // A face never shrinks below a cell, and the drag is refused whole rather than half-applied.
    expect(resizeSlot(rack, "a", "e", 6)).toBe(rack);
    expect(resizeSlot(rack, "a", "s", 2).slots?.[0]?.h).toBe(6);
    expect(resizeSlot(rack, "a", "s", 6)).toBe(rack);
    const stacked = placeSlot(pin(rack, "c"), "c", { x: 6, y: 4, w: 6, h: 4 });
    expect(resizeSlot(stacked, "a", "s", 1).slots).toEqual([
      { node: "a", x: 0, y: 0, w: 6, h: 5 },
      { node: "b", x: 6, y: 0, w: 6, h: 4 },
      { node: "c", x: 6, y: 4, w: 6, h: 4 },
    ]);
  });

  it("drops slots whose node is gone and re-places ones the grid no longer holds", () => {
    const rack = pin({ slots: [] }, "nfm");
    expect(pruneRack(rack, removeNode(workspace(), "nfm")).slots).toEqual([]);
    expect(pruneRack(rack, workspace())).toBe(rack);

    const stale = { slots: [{ node: "nfm", x: 12, y: 12, w: 12, h: 8 }] };
    expect(pruneRack(stale, workspace()).slots).toEqual([{ node: "nfm", x: 0, y: 0, w: 6, h: 4 }]);
  });
});
