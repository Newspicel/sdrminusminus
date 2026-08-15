import { describe, expect, it } from "vitest";
import type { AntennaGeometry, AntennaSegment } from "../../lib/types";
import {
  type Bounds,
  boundsOf,
  fitTo,
  place,
  planView,
  project,
  rolesIn,
  scaleBar,
  structureBounds,
} from "./geometry";

function segment(
  label: string,
  role: AntennaSegment["role"],
  from: [number, number, number],
  to: [number, number, number],
): AntennaSegment {
  return {
    label,
    role,
    from: { x_m: from[0], y_m: from[1], z_m: from[2] },
    to: { x_m: to[0], y_m: to[1], z_m: to[2] },
  };
}

const DIPOLE: AntennaGeometry = {
  feed: { x_m: 0, y_m: 0, z_m: 0 },
  segments: [
    segment("Leg", "driven", [-1, 0, 0], [0, 0, 0]),
    segment("Leg", "driven", [0, 0, 0], [1, 0, 0]),
  ],
};

const YAGI: AntennaGeometry = {
  feed: { x_m: 0, y_m: 0, z_m: 0.4 },
  segments: [
    segment("Boom", "structure", [0, 0, 0], [0, 0, 1.2]),
    segment("Reflector", "parasitic", [-0.53, 0, 0], [0.53, 0, 0]),
    segment("Driven element", "driven", [-0.5, 0, 0.4], [0.5, 0, 0.4]),
  ],
};

const VERTICAL: AntennaGeometry = {
  feed: { x_m: 0, y_m: 0, z_m: 0 },
  segments: [
    segment("Radiator", "driven", [0, 0, 0], [0, 1.2, 0]),
    segment("Radial", "radial", [0, 0, 0], [0.5, -0.5, 0]),
    segment("Radial", "radial", [0, 0, 0], [-0.5, -0.5, 0]),
  ],
};

describe("boundsOf", () => {
  it("measures every axis the antenna occupies, feedpoint included", () => {
    const bounds = boundsOf(YAGI);
    expect(bounds.x).toEqual({ min: -0.53, max: 0.53, size: 1.06 });
    expect(bounds.y).toEqual({ min: 0, max: 0, size: 0 });
    expect(bounds.z).toEqual({ min: 0, max: 1.2, size: 1.2 });
  });
});

describe("structureBounds", () => {
  /** The coax the quad's matching section is made of is cut to length, but the antenna is not
   * that much bigger for it. */
  it("leaves the feedline out of the antenna's own size", () => {
    const quad: AntennaGeometry = {
      feed: { x_m: 0, y_m: 0, z_m: 0 },
      segments: [
        segment("Side", "driven", [-0.5, 0, 0], [0.5, 0, 0]),
        segment("Side", "driven", [-0.5, 1, 0], [0.5, 1, 0]),
        segment("Matching line", "feedline", [0, 0, 0], [0, -0.7, 0]),
      ],
    };
    expect(structureBounds(quad).y).toEqual({ min: 0, max: 1, size: 1 });
    expect(boundsOf(quad).y.size).toBeCloseTo(1.7, 9);
  });
});

describe("planView", () => {
  it("draws a flat antenna face on", () => {
    expect(planView(boundsOf(DIPOLE)).label).toBe("Front view");
    expect(planView(boundsOf(VERTICAL)).label).toBe("Front view");
  });

  /** A Yagi seen from the front is a single element with the rest hidden behind it. */
  it("looks down on a boom, so the elements stay apart", () => {
    const view = planView(boundsOf(YAGI));
    expect(view.label).toBe("Top view");
    expect(view.horizontal).toBe("x");
    expect(view.vertical).toBe("z");
  });

  /** Looking down on a boom, the far end of it belongs at the top of the page. */
  it("points the boom away from the reader", () => {
    const view = planView(boundsOf(YAGI));
    const reflector = project({ x_m: 0, y_m: 0, z_m: 0 }, view.angles);
    const director = project({ x_m: 0, y_m: 0, z_m: 1.2 }, view.angles);
    expect(director.y).toBeLessThan(reflector.y);
  });

  it("takes the side when the antenna is deep and narrow", () => {
    const bounds: Bounds = {
      x: { min: 0, max: 0, size: 0 },
      y: { min: 0, max: 2, size: 2 },
      z: { min: 0, max: 3, size: 3 },
    };
    expect(planView(bounds).label).toBe("Side view");
  });
});

describe("project", () => {
  it("puts up on the screen's up", () => {
    const flat = project({ x_m: 2, y_m: 1, z_m: 0 }, { yaw: 0, pitch: 0 });
    expect(flat.x).toBeCloseTo(2, 9);
    expect(flat.y).toBeCloseTo(-1, 9);
  });

  it("turns depth into height when looking down", () => {
    const top = project({ x_m: 1, y_m: 5, z_m: 2 }, { yaw: 0, pitch: 90 });
    expect(top.x).toBeCloseTo(1, 9);
    expect(top.y).toBeCloseTo(2, 9);
  });

  it("swaps the boom into view from the side", () => {
    const side = project({ x_m: 3, y_m: 1, z_m: 2 }, { yaw: 90, pitch: 0 });
    expect(side.x).toBeCloseTo(2, 9);
    expect(side.y).toBeCloseTo(-1, 9);
  });

  it("keeps the origin at the origin from any angle", () => {
    const spun = project({ x_m: 0, y_m: 0, z_m: 0 }, { yaw: -32, pitch: 22 });
    expect(spun.x).toBeCloseTo(0, 12);
    expect(spun.y).toBeCloseTo(0, 12);
  });
});

describe("fitTo", () => {
  const viewport = { width: 200, height: 100, padding: 10 };

  it("fills the viewport without spilling out of the padding", () => {
    const fit = fitTo(
      [
        { x: -2, y: -1 },
        { x: 2, y: 1 },
      ],
      viewport,
    );
    const left = place({ x: -2, y: -1 }, fit);
    const right = place({ x: 2, y: 1 }, fit);
    expect(left.x).toBeCloseTo(20, 6);
    expect(right.x).toBeCloseTo(180, 6);
    expect(left.y).toBeCloseTo(10, 6);
    expect(right.y).toBeCloseTo(90, 6);
  });

  it("centres a drawing with no height at all", () => {
    const fit = fitTo(
      [
        { x: 0, y: 0 },
        { x: 4, y: 0 },
      ],
      viewport,
    );
    expect(place({ x: 0, y: 0 }, fit)).toEqual({ x: 10, y: 50 });
    expect(place({ x: 4, y: 0 }, fit)).toEqual({ x: 190, y: 50 });
  });

  it("has something to show before there is anything to draw", () => {
    expect(fitTo([], viewport)).toEqual({ scale: 1, offsetX: 100, offsetY: 50 });
  });
});

describe("scaleBar", () => {
  it("picks a round length that still fits", () => {
    expect(scaleBar(100, 250, "m")).toEqual({ meters: 2, pixels: 200, label: "2.000 m" });
    expect(scaleBar(1000, 120, "m")).toMatchObject({ meters: 0.1, pixels: 100 });
  });

  it("measures in feet when the table does", () => {
    const bar = scaleBar(100, 200, "ft");
    expect(bar.meters).toBeCloseTo(5 * 0.3048, 9);
    expect(bar.label).toBe("5 ft 0.0 in");
  });

  it("says nothing rather than a nonsense length when there is no scale", () => {
    expect(scaleBar(0, 200, "m").label).toBe("—");
  });
});

describe("rolesIn", () => {
  it("legends only the roles this drawing uses, in a fixed order", () => {
    expect(rolesIn(YAGI)).toEqual(["driven", "parasitic", "structure"]);
    expect(rolesIn(DIPOLE)).toEqual(["driven"]);
  });
});
