import { afterEach, describe, expect, it } from "vitest";
import { useFusionStore } from "./fusion";
import type { DfFusionState } from "./types";

const STATE: DfFusionState = { samples: 3 };

afterEach(() => useFusionStore.getState().reset());

describe("useFusionStore", () => {
  it("stores fusion updates by node and forgets them", () => {
    const store = useFusionStore.getState();
    store.observe({ type: "DfFusionUpdate", data: { node: "tri", state: STATE } });
    store.set("other", { samples: 9 });
    expect(useFusionStore.getState().byNode.tri?.samples).toBe(3);
    expect(useFusionStore.getState().byNode.other?.samples).toBe(9);
    store.forget(["tri"]);
    expect(useFusionStore.getState().byNode.tri).toBeUndefined();
    expect(useFusionStore.getState().byNode.other).toBeDefined();
  });

  it("ignores other events", () => {
    useFusionStore.getState().set("tri", STATE);
    useFusionStore
      .getState()
      .observe({ type: "SurfaceRefused", data: { node: "tri", reason: "no_surface" } });
    expect(useFusionStore.getState().byNode.tri).toBe(STATE);
  });

  it("drops what it held when the server greets again, so the seed can refill it", () => {
    useFusionStore.getState().set("tri", STATE);
    useFusionStore.getState().observe({ type: "Hello", data: { revision: 1, protocol: 1 } });
    expect(useFusionStore.getState().byNode).toEqual({});
  });
});
