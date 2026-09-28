import { beforeEach, describe, expect, it } from "vitest";
import { gridLocator, usePositionStore } from "./position";
import type { PositionFix, ServerEvent } from "./types";

function fix(latitude: number, over: Partial<PositionFix> = {}): PositionFix {
  return {
    latitude,
    longitude: 13.405,
    accuracy_m: 4,
    time: "2026-08-14T12:00:00Z",
    ...over,
  };
}

function event(position: PositionFix): ServerEvent {
  return {
    type: "PositionChanged",
    data: { node: "gps", fix: position },
  };
}

describe("gridLocator", () => {
  it("converts known station coordinates to six-character Maidenhead locators", () => {
    expect(gridLocator(52.52, 13.405)).toBe("JO62qm");
    expect(gridLocator(37.7749, -122.4194)).toBe("CM87ss");
  });

  it("keeps exact world edges inside the final field", () => {
    expect(gridLocator(90, 180)).toBe("RR99xx");
    expect(gridLocator(-90, -180)).toBe("AA00aa");
  });
});

describe("position history", () => {
  beforeEach(() => usePositionStore.getState().clear());

  it("replaces a duplicate location with its newest measurements", () => {
    usePositionStore.getState().observe(event(fix(52.52, { speed_mps: 1 })));
    usePositionStore.getState().observe(event(fix(52.52, { speed_mps: 2 })));
    const history = usePositionStore.getState().sources.gps?.history;
    expect(history).toHaveLength(1);
    expect(history?.[0]?.speed_mps).toBe(2);
  });

  it("caps each source at five thousand samples", () => {
    for (let latitude = 0; latitude <= 5_000; latitude += 1) {
      usePositionStore.getState().observe(event(fix(latitude)));
    }
    const history = usePositionStore.getState().sources.gps?.history;
    expect(history).toHaveLength(5_000);
    expect(history?.[0]?.latitude).toBe(1);
    expect(history?.at(-1)?.latitude).toBe(5_000);
  });
});
