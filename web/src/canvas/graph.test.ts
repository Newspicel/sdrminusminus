import { describe, expect, it } from "vitest";
import type {
  ChannelDescriptor,
  DeviceSet,
  PatchCatalog,
  PatchGraph,
  PatchNode,
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
  newNodeId,
  pin,
  placeSlot,
  portsOf,
  pruneRack,
  RACK_COLS,
  removeNode,
  resizeSlot,
  sameGraph,
  unpin,
} from "./graph";

// The catalog is generated; this is the shape `GET /api/patch/catalog` returns for the kinds the
// tests wire together.
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
          note: "reserved: transmit is not built (PLAN §12a)",
        },
        { name: "iq", port_type: "iq", direction: "out", multi: true },
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
];

const context: GraphContext = { catalog: CATALOG, channelTypes: TYPES };

function node(id: string, body: Partial<PatchNode> & Pick<PatchNode, "kind">): PatchNode {
  return { id, position: { x: 0, y: 0 }, ...body } as PatchNode;
}

function station(): PatchGraph {
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

/** The device node bound to a set running at this rate — all the live rate rule reads. */
const boundAt = (rate: number) =>
  new Map([["dev", { settings: { sample_rate: rate } } as DeviceSet]]);

describe("ports", () => {
  it("resolves a channel's conditional outputs against its type", () => {
    const graph = station();
    const nfm = graph.nodes[2];
    const adsb = node("adsb", { kind: "channel", data: { channel_type: "adsb" } });
    expect(nfm && portsOf(context, nfm).map((p) => p.name)).toEqual(["iq", "audio"]);
    expect(portsOf(context, adsb).map((p) => p.name)).toEqual(["iq", "events"]);
  });

  it("gives an unknown channel type only its input", () => {
    const ghost = node("x", { kind: "channel", data: { channel_type: "wefax" } });
    expect(portsOf(context, ghost).map((p) => p.name)).toEqual(["iq"]);
  });
});

describe("connectionRefusal", () => {
  it("accepts the wires the model is for", () => {
    const graph = station();
    expect(
      connectionRefusal(context, graph, port("nfm", "audio"), port("spk", "audio")),
    ).toBeNull();
    // A device fans out: a second scope on the same radio is the point.
    const withScope = { ...graph, nodes: [...graph.nodes, node("scope2", { kind: "scope" })] };
    expect(
      connectionRefusal(context, withScope, port("dev", "iq"), port("scope2", "iq")),
    ).toBeNull();
  });

  it("names the reason for every refusal", () => {
    const graph = station();
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

  // Two devices into one channel is refused until `CoherentArray` exists (PLAN §6).
  it("refuses a second device on a channel and names why", () => {
    const graph = {
      ...station(),
      nodes: [...station().nodes, node("dev2", { kind: "device", data: {} })],
    };
    expect(connectionRefusal(context, graph, port("dev2", "iq"), port("nfm", "iq"))).toMatch(
      /coherent array/,
    );
  });

  it("wires a scanner into the radio it drives, and only one", () => {
    const graph = {
      ...station(),
      nodes: [
        ...station().nodes,
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

  /** The transmit input is reserved (PLAN §12a): nothing emits its type, and what the operator
   * gets for trying is the server's own reason rather than a type-mismatch line. */
  it("refuses everything at the reserved transmit input, with the reason", () => {
    const graph = {
      ...station(),
      nodes: [...station().nodes, node("dev2", { kind: "device", data: {} })],
    };
    expect(connectionRefusal(context, graph, port("dev", "iq"), port("dev2", "tx"))).toMatch(
      /transmit is not built/,
    );
  });

  /// PLAN §18: the rate rule is a fault *on* the wire, not a refusal of it — the rate is one
  /// setting away, and the face at the end of the wire offers that setting.
  it("allows a wideband channel on a rate outside its range and marks the wire instead", () => {
    const graph = {
      ...station(),
      nodes: [
        ...station().nodes,
        node("adsb", { kind: "channel", data: { channel_type: "adsb" } }),
      ],
    };
    // Past the top of the range: ADS-B reads the radio's own samples, and above 4 Msps there is
    // nothing left for its slicer to gain.
    const wrong = { ...context, bound: boundAt(10_000_000) };
    expect(connectionRefusal(wrong, graph, port("dev", "iq"), port("adsb", "iq"))).toBeNull();

    // Short enough to sit on a wire; the face at its end carries the explanation.
    expect(edgeWarning(wrong, graph, port("dev", "iq"), port("adsb", "iq"))).toBe(
      "needs 2.000–4.000 MHz",
    );

    const right = { ...context, bound: boundAt(2_048_000) };
    expect(edgeWarning(right, graph, port("dev", "iq"), port("adsb", "iq"))).toBeNull();
    // An unbound device has no rate to be wrong about yet.
    expect(edgeWarning(context, graph, port("dev", "iq"), port("adsb", "iq"))).toBeNull();
    // A mode that leaves a guard band is never a fault, whatever the radio is doing.
    expect(edgeWarning(wrong, graph, port("dev", "iq"), port("nfm", "iq"))).toBeNull();
  });
});

describe("editing", () => {
  it("removing a node takes its wires with it", () => {
    const graph = removeNode(station(), "nfm");
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
          ...station().nodes,
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
    // Idempotent, and a station already in today's shape is returned untouched — the identity is
    // what keeps a read from invalidating every memo downstream of it.
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
    expect(sameGraph(station(), station())).toBe(true);
    expect(
      sameGraph(
        station(),
        addEdge(station(), {
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
    // Pinning twice is a no-op, not a second slot.
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
    // Resizing in place is allowed: a slot never collides with itself.
    expect(placeSlot(rack, "a", { x: 0, y: 0, w: 6, h: 8 }).slots?.[0]?.h).toBe(8);
  });

  it("trades places when a face is dropped on another", () => {
    // Two faces of different sizes, side by side.
    const rack = placeSlot(pin(pin({ slots: [] }, "a"), "b"), "b", { x: 6, y: 0, w: 6, h: 8 });
    const swapped = moveSlot(rack, "a", { x: 6, y: 0 });
    expect(swapped.slots).toEqual([
      { node: "a", x: 6, y: 0, w: 6, h: 8 },
      { node: "b", x: 0, y: 0, w: 6, h: 4 },
    ]);
    // Into free space it simply moves.
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
    // The same boundary from the other side, and back again.
    expect(resizeSlot(wider, "b", "w", -2)).toEqual(rack);
    // A face never shrinks below a cell, and the drag is refused whole rather than half-applied.
    expect(resizeSlot(rack, "a", "e", 6)).toBe(rack);
    // An edge with nothing behind it resizes this face alone, up to the grid.
    expect(resizeSlot(rack, "a", "s", 2).slots?.[0]?.h).toBe(6);
    expect(resizeSlot(rack, "a", "s", 6)).toBe(rack);
    // Faces that only touch at a corner do not share a boundary.
    const stacked = placeSlot(pin(rack, "c"), "c", { x: 6, y: 4, w: 6, h: 4 });
    expect(resizeSlot(stacked, "a", "s", 1).slots).toEqual([
      { node: "a", x: 0, y: 0, w: 6, h: 5 },
      { node: "b", x: 6, y: 0, w: 6, h: 4 },
      { node: "c", x: 6, y: 4, w: 6, h: 4 },
    ]);
  });

  it("drops slots whose node is gone and re-places ones the grid no longer holds", () => {
    const rack = pin({ slots: [] }, "nfm");
    expect(pruneRack(rack, removeNode(station(), "nfm")).slots).toEqual([]);
    expect(pruneRack(rack, station())).toBe(rack);

    // A rack stored against the old 24×24 grid: the slot is off this one, so it is re-placed
    // rather than left to make every later write fail validation.
    const stale = { slots: [{ node: "nfm", x: 12, y: 12, w: 12, h: 8 }] };
    expect(pruneRack(stale, station()).slots).toEqual([{ node: "nfm", x: 0, y: 0, w: 6, h: 4 }]);
  });
});
