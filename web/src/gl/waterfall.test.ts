import { afterEach, describe, expect, it, vi } from "vitest";
import { attachWaterfall, COLORMAPS, DEFAULT_COLORMAP } from "./waterfall";

describe("COLORMAPS", () => {
  it("holds the index order the shader switches on", () => {
    expect([...COLORMAPS]).toEqual(["classic", "magma", "inferno", "plasma", "viridis", "gray"]);
  });

  it("defaults to the map an unknown name also falls back to", () => {
    expect(COLORMAPS.indexOf(DEFAULT_COLORMAP)).toBe(0);
  });
});

describe("attachWaterfall", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("names the missing context where the system grants no WebGL2", () => {
    vi.stubGlobal("window", { clearTimeout: vi.fn() });
    vi.stubGlobal("document", { createElement: () => ({ getContext: () => null }) });

    expect(() => attachWaterfall({} as HTMLCanvasElement)).toThrow("no WebGL2 context");
  });
});
