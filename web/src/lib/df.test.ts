import { beforeEach, describe, expect, it } from "vitest";
import { useDfStore } from "./df";
import type { ServerEvent } from "./types";

function fused(node: string, samples: number): ServerEvent {
  return {
    type: "DfFusionUpdate",
    data: {
      node,
      state: {
        samples,
        estimate: {
          lat: 51.5,
          lon: 7,
          ellipse_major_m: 300,
          ellipse_minor_m: 200,
          ellipse_bearing_deg: 10,
          converged: true,
          samples,
        },
      },
    },
  };
}

describe("useDfStore", () => {
  beforeEach(() => {
    useDfStore.getState().reset();
  });

  it("keeps the newest fusion under its triangulation node", () => {
    const observe = useDfStore.getState().observe;
    observe(fused("cross", 2));
    observe(fused("cross", 4));
    const state = useDfStore.getState().byNode.cross;
    expect(state?.samples).toBe(4);
    expect(state?.estimate?.converged).toBe(true);
  });

  it("ignores events that are not fusion updates", () => {
    useDfStore.getState().observe({ type: "Hello", data: { revision: 1 } });
    expect(useDfStore.getState().byNode).toEqual({});
  });

  it("forgets a node the patch removed", () => {
    const store = useDfStore.getState();
    store.observe(fused("cross", 2));
    store.forget("cross");
    expect(useDfStore.getState().byNode.cross).toBeUndefined();
  });
});
