import { describe, expect, it } from "vitest";
import { type Measurable, offsetWithin } from "./portAnchor";

function box(
  offsetTop: number,
  offsetHeight: number,
  parent: Measurable | null,
  scrollTop = 0,
): Measurable {
  return { offsetTop, offsetHeight, scrollTop, offsetParent: parent, parentElement: parent };
}

describe("offsetWithin", () => {
  it("centres a row that sits directly in the node", () => {
    const node = box(0, 300, null);
    const row = box(120, 28, node);
    expect(offsetWithin(row, node)).toBe(134);
  });

  it("adds up the positioned ancestors between the row and the node", () => {
    const node = box(0, 300, null);
    const body = box(26, 200, node);
    const lanes = box(60, 100, body);
    const row = box(30, 28, lanes);
    expect(offsetWithin(row, node)).toBe(26 + 60 + 30 + 14);
  });

  it("takes a scrolled body into account", () => {
    const node = box(0, 300, null);
    const body = box(26, 200, node, 40);
    const row = box(100, 28, body);
    expect(offsetWithin(row, node)).toBe(26 + 100 + 14 - 40);
  });
});
