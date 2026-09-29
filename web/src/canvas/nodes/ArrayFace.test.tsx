import { afterEach, describe, expect, it } from "vitest";
import { useArrayStore } from "../../lib/arrays";
import type {
  ArrayNode,
  ArrayStatus,
  DeviceSet,
  PatchEdge,
  PatchGraph,
  PatchNode,
} from "../../lib/types";
import { catalogBody } from "../../test/catalog";
import { renderFace } from "../../test/faceHarness";
import { arrayStatus, capabilities, deviceSet, laneStatus, placed } from "../../test/fixtures";
import { streamPort } from "../graph";
import { ArrayFace } from "./ArrayFace";
import { ALIASING, NOT_WIRED, SAME_PLACE } from "./ArrayGeometryEditor";
import { NO_LANES_HINT } from "./ArrayLanes";
import { NO_HEADING } from "./ArraySettings";
import { PICK_ONE } from "./arrayNode";

const ARRAY: PatchNode = placed("north", { ...catalogBody("array"), label: "North" });

const KRAKEN = deviceSet({
  capabilities: capabilities({
    rx_streams: 5,
    coherence: "phase_coherent",
    freq_ranges: [{ min: 24e6, max: 1.766e9 }],
  }),
});

const RTL = deviceSet({ id: 2, device: { driver: "rtlsdr", key: "rtl", label: "RTL-SDR" } });

function wires(device: string, streams: number, from = 0): PatchEdge[] {
  return Array.from({ length: streams }, (_, index) => ({
    from: { node: device, port: streamPort("iq", index) },
    to: { node: "north", port: streamPort("lane", from + index) },
  }));
}

function graph(edges: PatchEdge[]): PatchGraph {
  return {
    nodes: [
      placed("kraken", {
        kind: "device",
        data: { device: { backend: "virtual", key: "kraken5" } },
      }),
      placed("rtl", { kind: "device", data: { device: { backend: "rtlsdr", key: "rtl" } } }),
      ARRAY,
    ],
    edges,
  };
}

const DEVICES = new Map([
  ["kraken", KRAKEN],
  ["rtl", RTL],
]);

function render(
  edges: PatchEdge[],
  status?: ArrayStatus,
  array: PatchNode = ARRAY,
  devices: ReadonlyMap<string, DeviceSet> = DEVICES,
): string {
  useArrayStore.setState({
    byNode: status === undefined ? {} : { north: status },
    receivedAt: status === undefined ? {} : { north: Date.now() },
  });
  const drawn = graph(edges);
  const nodes = drawn.nodes.map((node) => (node.id === array.id ? array : node));
  return renderFace(ArrayFace, array, { graph: { ...drawn, nodes }, devices });
}

function withData(data: Partial<ArrayNode>): PatchNode {
  return ARRAY.kind === "array" ? { ...ARRAY, data: { ...ARRAY.data, ...data } } : ARRAY;
}

function attributesOf(html: string, text: string): string {
  return new RegExp(`<(\\w+)([^<>]*)>${text}</\\1>`).exec(html)?.[2] ?? "";
}

function laneRows(html: string): number {
  return html.match(/<tr title="Lane \d/g)?.length ?? 0;
}

function fiveLanes(overrides: Partial<ArrayStatus> = {}): ArrayStatus {
  return arrayStatus("north", {
    lanes: [0, 1, 2, 3, 4].map((lane) => laneStatus(lane)),
    ...overrides,
  });
}

afterEach(() => useArrayStore.getState().reset());

describe("ArrayFace", () => {
  it("shows a row per wired lane and a Calibrate button", () => {
    const empty = render([]);
    expect(empty).toContain(NO_LANES_HINT);
    expect(laneRows(empty)).toBe(0);
    expect(empty).toContain("no lanes");
    expect(attributesOf(empty, "Calibrate")).toContain('disabled=""');

    const one = render(wires("kraken", 1));
    expect(laneRows(one)).toBe(1);
    expect(one).toContain("KrakenSDR iq1");

    const five = render(wires("kraken", 5), fiveLanes({ cal: "none" }));
    expect(laneRows(five)).toBe(5);
    expect(five).toContain("locked");
    expect(five).toContain("No cal");
    expect(attributesOf(five, "Calibrate")).toContain('title="Line up lane phase and gain"');
    expect(attributesOf(five, "Calibrate")).not.toContain('disabled=""');
    expect(five).toContain('aria-label="Lane tuning"');
    for (const section of ["Geometry", "Orientation", "Calibration", "Gain", "Tier"]) {
      expect(five).toMatch(new RegExp(`</span>${section}</\\w+>`));
    }
  });

  it("offers a tier choice only for several radios", () => {
    const one = render(wires("kraken", 5), fiveLanes({ tier: "phase_coherent" }));
    expect(one).not.toContain('aria-label="Array tier"');
    expect(one).toMatch(/>Radio<.*>Shared LO</);
    const two = render([...wires("kraken", 4), ...wires("rtl", 1, 4)], fiveLanes());
    expect(two).toContain('aria-label="Array tier"');
    expect(two).toContain("RTL-SDR iq");
  });

  it("shows gaps only when there are some", () => {
    const clean = render(wires("kraken", 5), fiveLanes());
    expect(clean).not.toContain(">Gaps<");
    expect(clean).not.toContain(">Drops<");
    const gappy = fiveLanes({
      lanes: [0, 1, 2, 3, 4].map((lane) => laneStatus(lane, { gaps: lane })),
      dropped_samples: 12,
    });
    const html = render(wires("kraken", 5), gappy);
    expect(html).toMatch(/>Gaps<.*>10</);
    expect(html).toContain(">Drops<");
  });

  it("fits a Kraken start delay in its column", () => {
    const spread = fiveLanes({
      lanes: [0, 1, 2, 3, 4].map((lane) => laneStatus(lane, { delay_samples: -16_803.25 * lane })),
    });
    const html = render(wires("kraken", 5), spread);
    expect(html).toMatch(/<td class="[^"]*truncate[^"]*">-67213<\/td>/);
    expect(html).not.toContain(">-67213.00<");
  });

  it("names a failure and flags aliasing above the element spacing", () => {
    const failed = fiveLanes({
      failure: { kind: "lane_held", lane: 2, by: "South" },
      center_hz: 433_920_000,
    });
    const html = render(wires("kraken", 5), failed);
    expect(html).toContain("failed");
    expect(html).toContain("Lane 3 in South");
    expect(html).toContain(ALIASING);
    expect(render(wires("kraken", 5), fiveLanes())).not.toContain(ALIASING);
  });

  it("spans spread lanes and hides the span when tuned together", () => {
    const spread = deviceSet({
      capabilities: capabilities({ rx_streams: 3, per_stream: { tuning: true } }),
      settings: {
        center_hz: 100_000_000,
        streams: [
          { stream: 1, center_hz: 102_000_000 },
          { stream: 2, center_hz: 104_000_000 },
        ],
      },
    });
    const devices = new Map([["kraken", spread]]);
    const status = fiveLanes({ tuning: "spread" });
    const html = render(wires("kraken", 3), status, withData({ tuning: "spread" }), devices);
    expect(html).toContain("Span 6.0480 MHz");
    expect(render(wires("kraken", 3), status, ARRAY, devices)).not.toContain("Span");
  });

  it("asks for a heading source when the array follows one", () => {
    const heading = withData({ orientation: { kind: "heading", mount_offset_deg: 0 } });
    const html = render(wires("kraken", 5), fiveLanes(), heading);
    expect(html).toContain(`>${NO_HEADING}<`);
    expect(html).toContain('title="Wire a GPS with heading, like a phone"');
    expect(render(wires("kraken", 5), fiveLanes())).not.toContain(`>${NO_HEADING}<`);
  });

  it("marks custom rows that are unwired or share a place, and offers Trim", () => {
    const custom = withData({
      geometry: {
        kind: "explicit",
        positions: [
          { x_m: 0, y_m: 0, z_m: 0 },
          { x_m: 0.3, y_m: 0, z_m: 0 },
          { x_m: 0.001, y_m: 0, z_m: 0 },
          { x_m: 0, y_m: 0.3, z_m: 0 },
        ],
      },
    });
    const html = render(wires("kraken", 3), fiveLanes(), custom);
    expect(html.match(new RegExp(`<tr title="${NOT_WIRED}"`, "g"))).toHaveLength(1);
    expect(html.match(new RegExp(`<tr title="${SAME_PLACE}"`, "g"))).toHaveLength(1);
    expect(html).toContain(">Trim<");
    expect(render(wires("kraken", 4), fiveLanes(), custom)).not.toContain(">Trim<");
  });

  it("asks to pick a tier and swaps Rec for Stop while recording", () => {
    const html = render(
      [...wires("kraken", 4), ...wires("rtl", 1, 4)],
      fiveLanes({
        recording: { stem: "north-1", started_at: "", samples: 4_096_000, dropped: 0 },
      }),
      withData({ declared: "none" }),
    );
    expect(html).toMatch(new RegExp(`aria-label="Array tier"[^>]*>.*${PICK_ONE}`));
    expect(html).toContain(">Stop<");
    expect(html).not.toContain(">Rec</button>");
    expect(html).toMatch(/>Rec<.*>2 s</);
  });
});
