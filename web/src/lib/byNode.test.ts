import { describe, expect, it } from "vitest";
import { shallow } from "zustand/shallow";
import { omitNodes, pickNodes } from "./byNode";

describe("pickNodes", () => {
  it("keeps only the listed nodes, so updates for other nodes compare equal", () => {
    const radar = { at: 1 };
    const before = { radar, spatial: { at: 1 } };
    const after = { ...before, spatial: { at: 2 }, other: { at: 3 } };
    expect(pickNodes(before, ["radar", "missing"])).toEqual({ radar });
    expect(shallow(pickNodes(before, ["radar"]), pickNodes(after, ["radar"]))).toBe(true);
    expect(shallow(pickNodes(before, ["spatial"]), pickNodes(after, ["spatial"]))).toBe(false);
  });
});

describe("omitNodes", () => {
  it("returns the same record when nothing listed is held", () => {
    const record = { a: 1 };
    expect(omitNodes(record, ["b"])).toBe(record);
    expect(omitNodes(record, ["a"])).toEqual({});
  });
});
