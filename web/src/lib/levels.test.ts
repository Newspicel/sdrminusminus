import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  FLUSH_MS,
  formatLevel,
  gateDb,
  gateOpen,
  LEVEL_FLOOR_DB,
  levelUnit,
  useLevelStore,
} from "./levels";
import type { ChannelLevel, ServerEvent } from "./types";

function update(deviceSet: number, levels: [number, number, number][]): ServerEvent {
  return {
    type: "ChannelLevels",
    data: {
      device_set: deviceSet,
      levels: levels.map(([channel, level_db, peak_db]) => ({ channel, level_db, peak_db })),
    },
  };
}

describe("useLevelStore", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    useLevelStore.getState().reset();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("publishes a reading once the flush interval has passed", () => {
    useLevelStore.getState().observe(update(1, [[7, -42, -30]]));
    expect(useLevelStore.getState().byDeviceSet).toEqual({});

    vi.advanceTimersByTime(FLUSH_MS);
    expect(useLevelStore.getState().byDeviceSet[1]?.[7]).toEqual({
      channel: 7,
      level_db: -42,
      peak_db: -30,
    });
  });

  it("collapses a burst of readings into one publication", () => {
    let renders = 0;
    const stop = useLevelStore.subscribe(() => {
      renders += 1;
    });
    for (let i = 0; i < 10; i++) {
      useLevelStore.getState().observe(update(1, [[7, -50 + i, -30]]));
    }
    vi.advanceTimersByTime(FLUSH_MS);
    stop();

    expect(renders).toBe(1);
    expect(useLevelStore.getState().byDeviceSet[1]?.[7]?.level_db).toBe(-41);
  });

  it("replaces a set's levels rather than merging them", () => {
    useLevelStore.getState().observe(
      update(1, [
        [7, -42, -30],
        [8, -60, -55],
      ]),
    );
    vi.advanceTimersByTime(FLUSH_MS);
    useLevelStore.getState().observe(update(1, [[7, -40, -30]]));
    vi.advanceTimersByTime(FLUSH_MS);

    expect(Object.keys(useLevelStore.getState().byDeviceSet[1] ?? {})).toEqual(["7"]);
  });

  it("keeps device sets apart", () => {
    useLevelStore.getState().observe(update(1, [[7, -42, -30]]));
    useLevelStore.getState().observe(update(2, [[7, -80, -70]]));
    vi.advanceTimersByTime(FLUSH_MS);

    expect(useLevelStore.getState().byDeviceSet[1]?.[7]?.level_db).toBe(-42);
    expect(useLevelStore.getState().byDeviceSet[2]?.[7]?.level_db).toBe(-80);
  });

  it("drops a set that has gone away, staged or published", () => {
    useLevelStore.getState().observe(update(1, [[7, -42, -30]]));
    vi.advanceTimersByTime(FLUSH_MS);
    useLevelStore.getState().observe(update(1, [[7, -41, -30]]));
    useLevelStore.getState().clear(1);
    vi.advanceTimersByTime(FLUSH_MS);

    expect(useLevelStore.getState().byDeviceSet[1]).toBeUndefined();
  });

  it("ignores events that are not levels", () => {
    useLevelStore.getState().observe({ type: "Hello", data: { revision: 1 } });
    vi.advanceTimersByTime(FLUSH_MS);
    expect(useLevelStore.getState().byDeviceSet).toEqual({});
  });
});

describe("levelUnit", () => {
  it("spans the floor to full scale", () => {
    expect(levelUnit(-90)).toBe(0);
    expect(levelUnit(-45)).toBeCloseTo(0.5, 6);
    expect(levelUnit(0)).toBe(1);
  });

  it("clamps outside the meter rather than running off it", () => {
    expect(levelUnit(-200)).toBe(0);
    expect(levelUnit(20)).toBe(1);
    expect(levelUnit(Number.NaN)).toBe(0);
  });

  it("takes a different floor when asked", () => {
    expect(levelUnit(-30, -60)).toBeCloseTo(0.5, 6);
  });
});

function reading(over: Partial<ChannelLevel> = {}): ChannelLevel {
  return { channel: 1, level_db: -50, peak_db: -40, ...over };
}

describe("the gate a level is measured against", () => {
  it("prefers the measured threshold to the setting", () => {
    expect(gateDb(reading({ squelch_db: -62 }), -100)).toBe(-62);
    expect(gateOpen(reading({ squelch_db: -62 }), -100)).toBe(true);
    expect(gateOpen(reading({ level_db: -70, squelch_db: -62 }), -100)).toBe(false);
  });

  it("falls back to the setting until a reading arrives", () => {
    expect(gateDb(undefined, -70)).toBe(-70);
    expect(gateOpen(undefined, -70)).toBe(false);
    expect(gateDb(reading(), -70)).toBe(-70);
  });

  it("has no gate at all where nothing states one", () => {
    expect(gateDb(reading(), null)).toBeNull();
    expect(gateDb(undefined, undefined)).toBeNull();
    expect(gateOpen(reading(), null)).toBe(false);
  });
});

describe("formatLevel", () => {
  it("prints one decimal of dB", () => {
    expect(formatLevel(-42.15)).toBe("-42.1 dB");
  });

  it("says nothing about a channel that has measured nothing", () => {
    expect(formatLevel(undefined)).toBe("—");
    expect(formatLevel(LEVEL_FLOOR_DB)).toBe("—");
    expect(formatLevel(Number.NEGATIVE_INFINITY)).toBe("—");
  });
});
