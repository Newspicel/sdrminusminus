import { describe, expect, it } from "vitest";
import { COLORMAPS, DEFAULT_COLORMAP } from "./waterfall";

/** `setColormap` sends `COLORMAPS.indexOf(name)` to the shader's `uMap`, which switches on the
 * literal integer. Nothing at runtime can notice a reordering: every plot simply draws in another
 * map's colours. The list is pinned here so the shader and this file can only move together. */
describe("COLORMAPS", () => {
  it("holds the index order the shader switches on", () => {
    expect([...COLORMAPS]).toEqual(["classic", "magma", "inferno", "plasma", "viridis", "gray"]);
  });

  it("defaults to the map an unknown name also falls back to", () => {
    // The shader's final `return` is the index-0 map, so a name it never heard of and the default
    // have to be the same one.
    expect(COLORMAPS.indexOf(DEFAULT_COLORMAP)).toBe(0);
  });
});
