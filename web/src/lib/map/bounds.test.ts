import { describe, expect, it } from "vitest";
import { trailBounds, unwrapTrail } from "./bounds";

describe("unwrapTrail", () => {
  it("keeps route segments local across the antimeridian", () => {
    expect(
      unwrapTrail([
        [179.8, 10],
        [-179.9, 11],
      ]),
    ).toEqual([
      [179.8, 10],
      [180.1, 11],
    ]);
  });
});

describe("trailBounds", () => {
  it("keeps an antimeridian crossing local", () => {
    expect(
      trailBounds([
        [179.8, 10],
        [-179.9, 11],
        [-179.7, 12],
      ]),
    ).toEqual([
      [179.8, 10],
      [180.3, 12],
    ]);
  });

  it("preserves ordinary bounds", () => {
    expect(
      trailBounds([
        [13.2, 52.1],
        [13.5, 52.6],
      ]),
    ).toEqual([
      [13.2, 52.1],
      [13.5, 52.6],
    ]);
  });
});
