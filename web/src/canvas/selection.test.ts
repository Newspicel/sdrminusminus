import { describe, expect, it } from "vitest";
import { focusNode, type Selectable } from "./selection";

function nodes(...entries: Selectable[]): Selectable[] {
  return entries;
}

describe("focusNode", () => {
  it("selects a node that has just arrived and clears the rest", () => {
    const before = nodes({ id: "a", selected: true }, { id: "b" }, { id: "fresh" });
    expect(focusNode(before, "fresh")).toEqual([
      { id: "a", selected: false },
      { id: "b" },
      { id: "fresh", selected: true },
    ]);
  });

  it("keeps a wider selection when the focus is already part of it", () => {
    const before = nodes({ id: "a", selected: true }, { id: "b", selected: true });
    expect(focusNode(before, "a")).toBe(before);
  });

  it("leaves the selection alone when nothing is focused", () => {
    const before = nodes({ id: "a", selected: true });
    expect(focusNode(before, null)).toBe(before);
  });

  it("waits for the focused node to mount before touching the selection", () => {
    const before = nodes({ id: "a", selected: true });
    expect(focusNode(before, "unmounted")).toBe(before);
  });

  it("keeps the identity of nodes it does not change", () => {
    const untouched = { id: "b" };
    const after = focusNode(nodes({ id: "a" }, untouched), "a");
    expect(after[1]).toBe(untouched);
    expect(after[0]).toEqual({ id: "a", selected: true });
  });
});
