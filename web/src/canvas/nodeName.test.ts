import { describe, expect, it } from "vitest";
import { nodeLabel } from "./nodeName";

describe("nodeLabel", () => {
  it("keeps a typed name without its edges", () => {
    expect(nodeLabel("  Desk audio  ", "Speaker")).toBe("Desk audio");
  });

  it("clears the label when nothing was typed", () => {
    expect(nodeLabel("", "Speaker")).toBeUndefined();
    expect(nodeLabel("   ", "Speaker")).toBeUndefined();
  });

  it("clears the label when the default title was typed", () => {
    expect(nodeLabel(" Speaker ", "Speaker")).toBeUndefined();
  });
});
