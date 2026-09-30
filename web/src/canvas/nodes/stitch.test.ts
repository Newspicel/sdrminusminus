import { describe, expect, it } from "vitest";
import type { PatchGraph } from "../../lib/types";
import { catalogBody } from "../../test/catalog";
import { placed } from "../../test/fixtures";
import { nodeOf } from "../graph";
import { needsSpread, spreadArrayEdit, stitchChips, stitchRow } from "./stitch";

function graph(wired: boolean): PatchGraph {
  return {
    nodes: [placed("arr", catalogBody("array")), placed("wide", catalogBody("stitch"))],
    edges: wired
      ? [{ from: { node: "arr", port: "array" }, to: { node: "wide", port: "array" } }]
      : [],
  };
}

describe("spreadArrayEdit", () => {
  it("spreads the wired array", () => {
    const next = spreadArrayEdit(graph(true), "wide");
    const array = next === null ? undefined : nodeOf(next, "arr");
    expect(array?.kind === "array" ? array.data.tuning : null).toBe("spread");
    expect(next === null ? true : needsSpread(next, "wide", null)).toBe(false);
  });

  it("returns null with no array", () => {
    expect(spreadArrayEdit(graph(false), "wide")).toBeNull();
  });

  it("asks for spread while the array tunes lanes together or the gate says so", () => {
    expect(needsSpread(graph(true), "wide", null)).toBe(true);
    expect(needsSpread(graph(false), "wide", null)).toBe(false);
    expect(needsSpread(graph(false), "wide", "tuning_mode")).toBe(true);
  });
});

describe("stitch rows", () => {
  it("formats a lane row", () => {
    const row = stitchRow({
      lane: 0,
      center_hz: 433_920_000,
      noise_eq_db: 0.44,
      phase_deg: 12.2,
      coherence: 0.823,
      spur_bins: 3,
    });
    expect(row).toMatchObject({ lane: "L1", mhz: "433.920", eq: "+0.4 dB", phase: "12°" });
    expect(row.title).toBe("L1 +0.4 dB 12° 0.82 3 spur bins");
    expect(stitchRow({ lane: 1, center_hz: 1e6, noise_eq_db: 0 }).coherence).toBe("-");
  });

  it("flags gaps and lost blocks", () => {
    const chips = stitchChips({
      at: "now",
      center_hz: 1,
      span_hz: 1,
      lanes: [],
      no_overlap: true,
      dropped_blocks: 4,
    });
    expect(chips.map((chip) => chip.label)).toEqual(["No overlap", "Drops 4"]);
    expect(stitchChips(null)).toEqual([]);
  });
});
