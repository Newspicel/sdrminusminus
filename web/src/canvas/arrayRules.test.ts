import { describe, expect, it } from "vitest";
import type { DeviceSet, PatchEdge, PatchGraph, PatchNode, PortRef } from "../lib/types";
import { CATALOG, catalogBody } from "../test/catalog";
import { capabilities, deviceSet } from "../test/fixtures";
import { arrayHoldingLane, lanesOf } from "./arrayRules";
import { connectionRefusal, type GraphContext } from "./graph";

function placed(id: string, kind: PatchNode["kind"], label?: string): PatchNode {
  const body = catalogBody(kind);
  return { id, position: { x: 0, y: 0 }, ...body, ...(label === undefined ? {} : { label }) };
}

function channel(id: string, channelType: string): PatchNode {
  return { id, position: { x: 0, y: 0 }, kind: "channel", data: { channel_type: channelType } };
}

function wire(from: string, fromPort: string, to: string, toPort: string): PatchEdge {
  return { from: { node: from, port: fromPort }, to: { node: to, port: toPort } };
}

const KRAKEN = deviceSet({
  capabilities: capabilities({ rx_streams: 5, coherence: "time_sync" }),
});
const RTL = deviceSet({ id: 2, capabilities: capabilities({ rx_streams: 2, coherence: "none" }) });

function context(bound: [string, DeviceSet][] = []): GraphContext {
  return {
    catalog: CATALOG,
    channelTypes: [
      {
        type_id: "adsb",
        name: "ADS-B",
        decoder_kind: "adsb",
      } as GraphContext["channelTypes"][number],
    ],
    facets: [],
    bound: new Map(bound),
  };
}

function graph(nodes: PatchNode[], edges: PatchEdge[] = []): PatchGraph {
  return { nodes, edges };
}

function refusal(
  patch: PatchGraph,
  from: PortRef,
  to: PortRef,
  bound: [string, DeviceSet][] = [],
): string | null {
  return connectionRefusal(context(bound), patch, from, to);
}

describe("array wiring rules", () => {
  it("accepts a Kraken lane into a free lane", () => {
    const patch = graph([placed("kraken", "device"), placed("arr", "array")]);
    expect(
      refusal(patch, { node: "kraken", port: "iq3" }, { node: "arr", port: "lane" }, [
        ["kraken", KRAKEN],
      ]),
    ).toBeNull();
  });

  it("accepts a second radio's lane into the same array", () => {
    const patch = graph(
      [placed("a", "device"), placed("b", "device"), placed("arr", "array")],
      [wire("a", "iq", "arr", "lane")],
    );
    expect(refusal(patch, { node: "b", port: "iq" }, { node: "arr", port: "lane2" })).toBeNull();
  });

  it("rule 1: an array lane takes a radio lane", () => {
    const patch = graph([placed("beam", "beamformer"), placed("arr", "array")]);
    expect(refusal(patch, { node: "beam", port: "beam" }, { node: "arr", port: "lane" })).toBe(
      "an array lane takes a radio lane",
    );
  });

  it("rule 2: a lane already in an array is refused, naming that array", () => {
    const patch = graph(
      [placed("kraken", "device"), placed("north", "array", "North"), placed("arr", "array")],
      [wire("kraken", "iq2", "north", "lane2")],
    );
    expect(refusal(patch, { node: "kraken", port: "iq2" }, { node: "arr", port: "lane" })).toBe(
      "that lane is in North",
    );
    expect(refusal(patch, { node: "kraken", port: "iq2" }, { node: "north", port: "lane" })).toBe(
      "that lane is in North",
    );
    const unnamed = graph(
      [placed("kraken", "device"), placed("first", "array"), placed("arr", "array")],
      [wire("kraken", "iq", "first", "lane")],
    );
    expect(refusal(unnamed, { node: "kraken", port: "iq" }, { node: "arr", port: "lane" })).toBe(
      "that lane is in an Array",
    );
    expect(refusal(unnamed, { node: "kraken", port: "iq" }, { node: "first", port: "lane" })).toBe(
      "already wired",
    );
  });

  it("rule 3: two lanes of a radio without a shared clock", () => {
    const patch = graph(
      [placed("rtl", "device"), placed("arr", "array")],
      [wire("rtl", "iq", "arr", "lane")],
    );
    expect(
      refusal(patch, { node: "rtl", port: "iq2" }, { node: "arr", port: "lane2" }, [["rtl", RTL]]),
    ).toBe("this radio's lanes share no clock");
    expect(
      refusal(patch, { node: "rtl", port: "iq2" }, { node: "arr", port: "lane2" }, [
        ["rtl", KRAKEN],
      ]),
    ).toBeNull();
  });

  it("rule 4: steer takes a direction finder", () => {
    const patch = graph([placed("tri", "triangulation"), placed("beam", "beamformer")]);
    expect(refusal(patch, { node: "tri", port: "events" }, { node: "beam", port: "steer" })).toBe(
      "steer takes a direction finder",
    );
    const good = graph([placed("df", "df"), placed("beam", "beamformer")]);
    expect(
      refusal(good, { node: "df", port: "events" }, { node: "beam", port: "steer" }),
    ).toBeNull();
  });

  it("rule 5: adsb takes ADS-B events", () => {
    const patch = graph([
      placed("df", "df"),
      channel("adsb", "adsb"),
      placed("filter", "event_filter"),
      placed("radar", "passive_radar"),
    ]);
    expect(refusal(patch, { node: "df", port: "events" }, { node: "radar", port: "adsb" })).toBe(
      "adsb takes ADS-B events",
    );
    expect(
      refusal(patch, { node: "adsb", port: "events" }, { node: "radar", port: "adsb" }),
    ).toBeNull();
    expect(
      refusal(patch, { node: "filter", port: "events" }, { node: "radar", port: "adsb" }),
    ).toBeNull();
  });

  it("rule 6: triangulation takes bearings", () => {
    const patch = graph([
      channel("adsb", "adsb"),
      placed("hunt", "hunt"),
      placed("tri", "triangulation"),
    ]);
    expect(refusal(patch, { node: "adsb", port: "events" }, { node: "tri", port: "events" })).toBe(
      "triangulation takes bearings",
    );
    expect(
      refusal(patch, { node: "hunt", port: "events" }, { node: "tri", port: "events" }),
    ).toBeNull();
  });
});

describe("array lanes", () => {
  it("lists the lanes of an array in lane order and who holds a radio lane", () => {
    const patch = graph(
      [placed("kraken", "device"), placed("arr", "array")],
      [wire("kraken", "iq3", "arr", "lane3"), wire("kraken", "iq", "arr", "lane")],
    );
    expect(lanesOf(patch, "arr").map((lane) => [lane.lane, lane.port, lane.source.port])).toEqual([
      [0, "lane", "iq"],
      [2, "lane3", "iq3"],
    ]);
    expect(arrayHoldingLane(patch, { node: "kraken", port: "iq3" })).toBe("arr");
    expect(arrayHoldingLane(patch, { node: "kraken", port: "iq2" })).toBeNull();
  });
});
