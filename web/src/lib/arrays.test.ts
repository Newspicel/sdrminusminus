import { afterEach, describe, expect, it } from "vitest";
import { arrayStatus } from "../test/fixtures";
import { failureText, GATE_TEXT, processorStatusOf, SYNC_TEXT, useArrayStore } from "./arrays";

afterEach(() => useArrayStore.getState().reset());

describe("useArrayStore", () => {
  it("seeds from the snapshot and updates from events", () => {
    const store = useArrayStore.getState();
    store.seed([arrayStatus("a", { sync: "searching" }), arrayStatus("b")]);
    expect(Object.keys(useArrayStore.getState().byNode)).toEqual(["a", "b"]);
    store.observe({
      type: "ArrayUpdate",
      data: { status: arrayStatus("a", { sync: "locked", realigns: 2 }) },
    });
    const a = useArrayStore.getState().byNode.a;
    expect(a?.sync).toBe("locked");
    expect(a?.realigns).toBe(2);
    expect(useArrayStore.getState().receivedAt.a).toBeGreaterThan(0);
  });

  it("forgets an array", () => {
    const store = useArrayStore.getState();
    store.seed([arrayStatus("a"), arrayStatus("b")]);
    store.forget(["a"]);
    expect(useArrayStore.getState().byNode.a).toBeUndefined();
    expect(useArrayStore.getState().receivedAt.a).toBeUndefined();
    expect(useArrayStore.getState().byNode.b).toBeDefined();
  });
});

describe("array labels", () => {
  it("takes status labels from the generated table", () => {
    expect(SYNC_TEXT.searching).toBe("Syncing");
    expect(GATE_TEXT.phase).toBe("Needs cal");
  });

  it("fills failure templates with one-based lanes", () => {
    expect(failureText({ kind: "lane_gap", lane: 1 })).toBe("Lane 2 unwired");
    expect(failureText({ kind: "lane_held", lane: 0, by: "North" })).toBe("Lane 1 in North");
    expect(failureText({ kind: "clock_drift", ppm: 1.234 })).toBe("Clocks drift 1.2 ppm");
    expect(failureText({ kind: "geometry_mismatch", positions: 4, lanes: 5 })).toBe(
      "Geometry has 4, wired 5",
    );
    expect(failureText({ kind: "unwired" })).toBe("Wire lanes");
  });

  it("finds a processor's status on its array", () => {
    const status = arrayStatus("arr", {
      processors: [
        {
          node: "df",
          kind: "df",
          running: true,
          gated: "calibrating",
          gated_samples: 0,
          dropped_samples: 0,
          dropped_reports: 0,
          lane_overflows: 0,
          lane_mismatch: 0,
          solver_failures: 0,
          resets: 0,
        },
      ],
    });
    expect(processorStatusOf(status, "df")?.gated).toBe("calibrating");
    expect(processorStatusOf(status, "other")).toBeNull();
    expect(processorStatusOf(undefined, "df")).toBeNull();
  });
});
