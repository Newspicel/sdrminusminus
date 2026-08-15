import { describe, expect, it } from "vitest";
import { COLORMAP_GLSL, COLORMAPS, sampleColormap } from "./colormap";

describe("sampleColormap", () => {
  it("stays inside the unit cube across every ramp", () => {
    for (const map of COLORMAPS) {
      for (let i = 0; i <= 64; i++) {
        for (const channel of sampleColormap(map, i / 64)) {
          expect(channel).toBeGreaterThanOrEqual(0);
          expect(channel).toBeLessThanOrEqual(1);
        }
      }
    }
  });

  it("clamps out-of-range and non-finite inputs to the ramp's ends", () => {
    expect(sampleColormap("gray", -1)).toEqual(sampleColormap("gray", 0));
    expect(sampleColormap("gray", 2)).toEqual(sampleColormap("gray", 1));
    expect(sampleColormap("gray", Number.NaN)).toEqual(sampleColormap("gray", 0));
  });

  it("reads classic's endpoints as its first and last stop", () => {
    expect(sampleColormap("classic", 0)).toEqual([0, 0, 0.12549]);
    expect(sampleColormap("classic", 1)).toEqual([0.2902, 0, 0]);
  });

  it("rises monotonically through gray", () => {
    let previous = -1;
    for (let i = 0; i <= 32; i++) {
      const [value] = sampleColormap("gray", i / 32);
      expect(value).toBeGreaterThan(previous);
      previous = value;
    }
  });

  it("gives every ramp a distinct midpoint", () => {
    const seen = new Set(COLORMAPS.map((map) => sampleColormap(map, 0.5).join()));
    expect(seen.size).toBe(COLORMAPS.length);
  });
});

describe("COLORMAP_GLSL", () => {
  it("branches on each polynomial ramp's index", () => {
    for (const map of ["magma", "inferno", "plasma", "viridis"] as const) {
      expect(COLORMAP_GLSL).toContain(`if (uMap == ${COLORMAPS.indexOf(map)}) { return poly(t,`);
    }
    expect(COLORMAP_GLSL).toContain(
      `if (uMap == ${COLORMAPS.indexOf("gray")}) { return vec3(t); }`,
    );
  });

  it("declares one array entry per classic stop", () => {
    expect(COLORMAP_GLSL).toContain("const vec3 CLASSIC[15] = vec3[15](");
    expect(COLORMAP_GLSL).toContain("vec3(0.00000000, 0.00000000, 0.12549000)");
  });
});
