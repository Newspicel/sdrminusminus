import { afterEach, describe, expect, it } from "vitest";
import { cellKey, useSurveyStore, withCell } from "./survey";
import type { SurveyCell, SurveyUpdate } from "./types";

function cell(latitude: number, level: number, frequency = 145_000_000): SurveyCell {
  return {
    latitude,
    longitude: 13,
    frequency_hz: frequency,
    level_dbfs: level,
    measured_at: "2026-09-28T12:00:00Z",
    observations: 1,
  };
}

function update(overrides: Partial<SurveyUpdate>): SurveyUpdate {
  return { cells: 0, dropped: 0, recording: true, ...overrides };
}

afterEach(() => useSurveyStore.getState().reset());

describe("survey cells", () => {
  it("keys cells the way the server merges them", () => {
    expect(cellKey(cell(52, -40))).toBe(cellKey(cell(52.00001, -30)));
    expect(cellKey(cell(52, -40))).not.toBe(cellKey(cell(52.001, -40)));
    expect(cellKey(cell(52, -40))).not.toBe(cellKey(cell(52, -40, 146_000_000)));
  });

  it("replaces a changed cell and drops the oldest when the server evicts", () => {
    const cells = [cell(52, -40), cell(52.01, -41)];
    expect(withCell(cells, cell(52.00001, -30), 2).map((c) => c.level_dbfs)).toEqual([-30, -41]);
    expect(withCell(cells, cell(52.02, -42), 3).map((c) => c.level_dbfs)).toEqual([-40, -41, -42]);
    expect(withCell(cells, cell(52.02, -42), 2).map((c) => c.level_dbfs)).toEqual([-41, -42]);
  });
});

describe("useSurveyStore", () => {
  it("seeds from the grid and follows updates", () => {
    const store = useSurveyStore.getState();
    store.seed({
      node: "map",
      bandwidth_hz: 12_500,
      offset_hz: 0,
      recording: false,
      cells: [cell(52, -40)],
    });
    store.observe({
      type: "SurveyUpdate",
      data: { node: "map", update: update({ cell: cell(52.01, -35), level_dbfs: -35, cells: 2 }) },
    });
    const state = useSurveyStore.getState().byNode.map;
    expect(state?.cells).toHaveLength(2);
    expect(state?.levelDbfs).toBe(-35);
    expect(state?.recording).toBe(true);
    store.observe({
      type: "SurveyUpdate",
      data: { node: "map", update: update({ recording: false, stopped: "retuned", cells: 2 }) },
    });
    expect(useSurveyStore.getState().byNode.map?.stopped).toBe("retuned");
    expect(useSurveyStore.getState().byNode.map?.cells).toHaveLength(2);
    store.observe({
      type: "SurveyUpdate",
      data: { node: "map", update: update({ recording: false, cells: 2, level_dbfs: -50 }) },
    });
    expect(useSurveyStore.getState().byNode.map?.stopped).toBe("retuned");
    store.observe({
      type: "SurveyUpdate",
      data: { node: "map", update: update({ recording: true, cells: 2 }) },
    });
    expect(useSurveyStore.getState().byNode.map?.stopped).toBeNull();
  });

  it("follows the server's cell count when another client clears", () => {
    const store = useSurveyStore.getState();
    store.seed({
      node: "map",
      bandwidth_hz: 12_500,
      offset_hz: 0,
      recording: true,
      cells: [cell(52, -40), cell(52.01, -41)],
    });
    store.observe({ type: "SurveyUpdate", data: { node: "map", update: update({ cells: 0 }) } });
    expect(useSurveyStore.getState().byNode.map?.cells).toEqual([]);
    store.observe({
      type: "SurveyUpdate",
      data: { node: "map", update: update({ cell: cell(52.03, -45), cells: 1 }) },
    });
    expect(useSurveyStore.getState().byNode.map?.cells.map((c) => c.level_dbfs)).toEqual([-45]);
  });
});
