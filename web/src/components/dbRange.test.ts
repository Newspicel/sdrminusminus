import { describe, expect, it } from "vitest";
import { clampWindow, DB_LIMIT, DB_MIN_SPAN, withCeiling, withFloor } from "./dbRange";

describe("withFloor", () => {
  it("moves the floor and leaves the ceiling alone", () => {
    expect(withFloor({ min: -110, max: -40 }, -95)).toEqual({ min: -95, max: -40 });
  });

  it("pushes the ceiling ahead of a floor that would pass it", () => {
    expect(withFloor({ min: -110, max: -40 }, -20)).toEqual({
      min: -20,
      max: -20 + DB_MIN_SPAN,
    });
  });

  it("holds the floor inside the limits", () => {
    expect(withFloor({ min: -110, max: -40 }, -400).min).toBe(DB_LIMIT.min);
    expect(withFloor({ min: -110, max: -40 }, 900).min).toBe(DB_LIMIT.max - DB_MIN_SPAN);
  });

  it("rounds to whole decibels", () => {
    expect(withFloor({ min: -110, max: -40 }, -94.6).min).toBe(-95);
  });
});

describe("withCeiling", () => {
  it("moves the ceiling and leaves the floor alone", () => {
    expect(withCeiling({ min: -110, max: -40 }, -60)).toEqual({ min: -110, max: -60 });
  });

  it("pushes the floor below a ceiling that would pass it", () => {
    expect(withCeiling({ min: -110, max: -40 }, -120)).toEqual({
      min: -120 - DB_MIN_SPAN,
      max: -120,
    });
  });

  it("holds the ceiling inside the limits", () => {
    expect(withCeiling({ min: -110, max: -40 }, 900).max).toBe(DB_LIMIT.max);
    expect(withCeiling({ min: -110, max: -40 }, -400).max).toBe(DB_LIMIT.min + DB_MIN_SPAN);
  });
});

describe("clampWindow", () => {
  it("passes a window that already fits", () => {
    expect(clampWindow({ min: -110, max: -40 })).toEqual({ min: -110, max: -40 });
  });

  it("brings a silent frame's window back inside the limits", () => {
    const window = clampWindow({ min: -250, max: -180 });
    expect(window.min).toBeGreaterThanOrEqual(DB_LIMIT.min);
    expect(window.max - window.min).toBeGreaterThanOrEqual(DB_MIN_SPAN);
  });

  it("replaces levels that are not numbers", () => {
    expect(clampWindow({ min: Number.NEGATIVE_INFINITY, max: Number.NaN })).toEqual({
      min: DB_LIMIT.min,
      max: DB_LIMIT.max,
    });
  });
});
