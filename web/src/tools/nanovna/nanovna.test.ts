import { describe, expect, it } from "vitest";
import type { NanoVnaSweep, ToolResponse } from "../../lib/types";
import {
  formatImpedance,
  gainDb,
  impedance,
  lowestVswrIndex,
  nanoVnaDevices,
  nanoVnaDevicesRequest,
  nanoVnaSweep,
  nanoVnaSweepRequest,
  phaseDeg,
  returnLossDb,
  vswr,
} from "./nanovna";

describe("NanoVNA requests", () => {
  it("tags discovery and sweep calls with the tool id", () => {
    expect(nanoVnaDevicesRequest()).toEqual({
      tool: "nanovna",
      request: { action: "list_devices" },
    });
    expect(
      nanoVnaSweepRequest({
        port: "COM3",
        start_hz: 1_000_000,
        stop_hz: 30_000_000,
        points: 101,
        averages: 2,
      }),
    ).toEqual({
      tool: "nanovna",
      request: {
        action: "sweep",
        port: "COM3",
        start_hz: 1_000_000,
        stop_hz: 30_000_000,
        points: 101,
        averages: 2,
      },
    });
  });

  it("unwraps only the matching NanoVNA result", () => {
    const devices: ToolResponse = {
      tool: "nanovna",
      result: { kind: "devices", devices: [] },
    };
    expect(nanoVnaDevices(devices)).toEqual([]);
    expect(nanoVnaSweep(devices)).toBeNull();
  });
});

describe("RF measurements", () => {
  it("derives matched-load measurements analytically", () => {
    const matched = { re: 0, im: 0 };
    expect(vswr(matched)).toBe(1);
    expect(impedance(matched)).toEqual({ re: 50, im: 0 });
    expect(returnLossDb(matched)).toBe(Number.POSITIVE_INFINITY);
  });

  it("derives magnitude, phase, VSWR, and impedance", () => {
    const gamma = { re: 0, im: 0.5 };
    expect(gainDb(gamma)).toBeCloseTo(-6.0206, 4);
    expect(returnLossDb(gamma)).toBeCloseTo(6.0206, 4);
    expect(phaseDeg(gamma)).toBe(90);
    expect(vswr(gamma)).toBe(3);
    expect(impedance(gamma)).toEqual({ re: 30, im: 40 });
    expect(formatImpedance(impedance(gamma))).toBe("30.0 + j40.0 Ω");
  });

  it("finds the best match in a sweep", () => {
    const sweep: NanoVnaSweep = {
      port: "COM3",
      firmware: "0.7.2",
      requested_points: 3,
      averages: 1,
      points: [
        { frequency_hz: 1, s11: { re: 0.5, im: 0 }, s21: { re: 0, im: 0 } },
        { frequency_hz: 2, s11: { re: 0.1, im: 0 }, s21: { re: 0, im: 0 } },
        { frequency_hz: 3, s11: { re: 0.3, im: 0 }, s21: { re: 0, im: 0 } },
      ],
    };
    expect(lowestVswrIndex(sweep.points)).toBe(1);
  });
});
