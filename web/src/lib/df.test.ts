import { beforeEach, describe, expect, it } from "vitest";
import { BEARING_HISTORY, useDfStore } from "./df";
import type { CalState, DfReading, ServerEvent } from "./types";

function reading(bearing: number, confidence = 0.8): DfReading {
  return {
    bearing_deg: bearing,
    confidence,
    peak_to_floor_db: 20,
    pseudospectrum: [1, 2, 3],
  };
}

function cal(over: Partial<CalState> = {}): CalState {
  return { tier: "phase_coherent", lanes: [], phase_unknown: false, solved: true, ...over };
}

function update(bearing: number, confidence = 0.8): ServerEvent {
  return {
    type: "DfUpdate",
    data: { device_set: 1, node: "df", reading: reading(bearing, confidence), cal: cal() },
  };
}

describe("useDfStore", () => {
  beforeEach(() => {
    useDfStore.getState().reset();
  });

  it("keeps the newest reading and the trail behind it", () => {
    const observe = useDfStore.getState().observe;
    observe(update(10));
    observe(update(20));
    const state = useDfStore.getState().byNode.df;
    expect(state?.reading.bearing_deg).toBe(20);
    expect(state?.history.map((sample) => sample.bearingDeg)).toEqual([10, 20]);
    expect(state?.deviceSet).toBe(1);
  });

  it("leaves the trail alone for a reading with no confidence in it", () => {
    const observe = useDfStore.getState().observe;
    observe(update(10));
    observe(update(0, 0));
    expect(useDfStore.getState().byNode.df?.history).toHaveLength(1);
  });

  it("caps the trail so a long drive cannot grow without bound", () => {
    const observe = useDfStore.getState().observe;
    for (let index = 0; index < BEARING_HISTORY + 20; index++) {
      observe(update(index % 360));
    }
    expect(useDfStore.getState().byNode.df?.history).toHaveLength(BEARING_HISTORY);
  });

  it("keeps the last bearing when only the fusion moves", () => {
    const observe = useDfStore.getState().observe;
    observe(update(30));
    observe({
      type: "DfFusionUpdate",
      data: {
        node: "df",
        state: {
          samples: 4,
          estimate: {
            lat: 51.5,
            lon: 7,
            ellipse_major_m: 300,
            ellipse_minor_m: 200,
            ellipse_bearing_deg: 10,
            converged: true,
            samples: 4,
          },
        },
      },
    });
    const state = useDfStore.getState().byNode.df;
    expect(state?.reading.bearing_deg).toBe(30);
    expect(state?.fusion?.estimate?.converged).toBe(true);
  });

  it("keeps radar detections under the node that found them", () => {
    useDfStore.getState().observe({
      type: "RadarDetections",
      data: {
        device_set: 2,
        node: "radar",
        detections: [{ range_bin: 60, range_km: 18, doppler_hz: 120, snr_db: 19 }],
      },
    });
    expect(useDfStore.getState().byNode.radar?.detections).toHaveLength(1);
  });

  it("forgets a node the patch removed", () => {
    const store = useDfStore.getState();
    store.observe(update(10));
    store.forget("df");
    expect(useDfStore.getState().byNode.df).toBeUndefined();
  });
});
