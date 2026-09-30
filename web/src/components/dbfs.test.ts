import { describe, expect, it } from "vitest";
import {
  formatPeak,
  HEADROOM,
  METER_FLOOR_DB,
  METER_HOT_DB,
  METER_WARN_DB,
  meterTone,
  meterUnit,
} from "./dbfs";

describe("meterUnit", () => {
  it("spans the floor to full scale", () => {
    expect(meterUnit(METER_FLOOR_DB)).toBe(0);
    expect(meterUnit(-30)).toBeCloseTo(0.5);
    expect(meterUnit(0)).toBe(1);
  });

  it("pins silence and overdrive to the ends", () => {
    expect(meterUnit(undefined)).toBe(0);
    expect(meterUnit(-140)).toBe(0);
    expect(meterUnit(Number.NEGATIVE_INFINITY)).toBe(0);
    expect(meterUnit(3)).toBe(1);
  });

  it("marks headroom where the tone changes", () => {
    expect(HEADROOM.warn).toBeCloseTo(meterUnit(METER_WARN_DB));
    expect(HEADROOM.hot).toBeCloseTo(meterUnit(METER_HOT_DB));
    expect(HEADROOM.warn).toBeLessThan(HEADROOM.hot);
  });
});

describe("meterTone", () => {
  it("warns near the top and goes red at the rail", () => {
    expect(meterTone(-30, false)).toBe("ok");
    expect(meterTone(METER_WARN_DB, false)).toBe("warn");
    expect(meterTone(METER_HOT_DB, false)).toBe("danger");
  });

  it("goes red whenever the radio reports clipping", () => {
    expect(meterTone(-40, true)).toBe("danger");
    expect(meterTone(undefined, true)).toBe("danger");
    expect(meterTone(undefined, false)).toBe("ok");
  });
});

describe("formatPeak", () => {
  it("names silence and rounds a reading", () => {
    expect(formatPeak(undefined)).toBe("quiet");
    expect(formatPeak(-140)).toBe("quiet");
    expect(formatPeak(-6.04)).toBe("-6.0 dBFS");
  });
});
