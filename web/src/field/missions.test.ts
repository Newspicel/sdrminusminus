import { describe, expect, it } from "vitest";
import type { PatchGraph } from "../lib/types";
import type { Mission } from "./missions";
import { fieldPath, isFieldPath, missionTargets, parseFieldPath } from "./missions";

const MISSIONS: Mission[] = [
  {
    id: "foxhunt",
    title: "Fox hunt",
    blurb: "",
    nodeKind: "channel",
    component: () => null,
  },
  { id: "df", title: "DF drive", blurb: "", nodeKind: "df", component: () => null },
];

function graph(): PatchGraph {
  return {
    nodes: [
      { id: "voice", kind: "channel", data: { channel_type: "nfm" }, position: { x: 0, y: 0 } },
      { id: "array", kind: "df", data: {}, label: "Roof array", position: { x: 0, y: 0 } },
      { id: "map", kind: "map", position: { x: 0, y: 0 } },
    ],
    edges: [],
  };
}

describe("parseFieldPath", () => {
  it("reads the mission and node out of the address", () => {
    expect(parseFieldPath("/field")).toEqual({ mission: null, node: null });
    expect(parseFieldPath("/field/")).toEqual({ mission: null, node: null });
    expect(parseFieldPath("/field/df/array")).toEqual({ mission: "df", node: "array" });
    expect(parseFieldPath("/field/df/a%20node")).toEqual({ mission: "df", node: "a node" });
  });

  it("says nothing about an address that is not field mode", () => {
    expect(parseFieldPath("/")).toEqual({ mission: null, node: null });
    expect(parseFieldPath("/fields/df")).toEqual({ mission: null, node: null });
  });
});

describe("isFieldPath", () => {
  it("claims only the field route", () => {
    expect(isFieldPath("/field")).toBe(true);
    expect(isFieldPath("/field/df/array")).toBe(true);
    expect(isFieldPath("/")).toBe(false);
    expect(isFieldPath("/fieldwork")).toBe(false);
  });
});

describe("fieldPath", () => {
  it("round-trips a node whose name needs escaping", () => {
    const path = fieldPath("df", "a node");
    expect(parseFieldPath(path)).toEqual({ mission: "df", node: "a node" });
  });
});

describe("missionTargets", () => {
  it("offers each mission only the nodes it can drive", () => {
    const targets = missionTargets(graph(), MISSIONS);
    expect(targets.map((target) => `${target.mission.id}:${target.node}`)).toEqual([
      "foxhunt:voice",
      "df:array",
    ]);
    expect(targets[1]?.label).toBe("Roof array");
    expect(targets[0]?.label).toBe("voice");
  });

  it("offers nothing for an empty patch", () => {
    expect(missionTargets({ nodes: [], edges: [] }, MISSIONS)).toEqual([]);
  });
});
