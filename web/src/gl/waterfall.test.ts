import { describe, expect, it } from "vitest";
import { COLORMAPS, DEFAULT_COLORMAP } from "./waterfall";

describe("COLORMAPS", () => {
  it("holds the index order the shader switches on", () => {
    expect([...COLORMAPS]).toEqual(["classic", "magma", "inferno", "plasma", "viridis", "gray"]);
  });

  it("defaults to the map an unknown name also falls back to", () => {
    expect(COLORMAPS.indexOf(DEFAULT_COLORMAP)).toBe(0);
  });
});
