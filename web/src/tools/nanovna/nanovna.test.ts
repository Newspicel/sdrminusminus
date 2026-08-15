import { describe, expect, it } from "vitest";
import type { ToolResponse } from "../../lib/types";
import {
  equivalentComponent,
  formatImpedance,
  formatSi,
  gainDb,
  groupDelays,
  impedance,
  lowestVswrIndex,
  mismatchLossDb,
  nanoVnaCalibrateRequest,
  nanoVnaDescribeRequest,
  nanoVnaDevices,
  nanoVnaDevicesRequest,
  nanoVnaIgnoredPorts,
  nanoVnaSweep,
  nanoVnaSweepRequest,
  phaseDeg,
  qFactor,
  returnLossDb,
  unwrappedPhase,
  vswr,
} from "./nanovna";
import { point, sweepOf } from "./testdata";

describe("NanoVNA requests", () => {
  it("tags every call with the tool id", () => {
    expect(nanoVnaDevicesRequest()).toEqual({
      tool: "nanovna",
      request: { action: "list_devices" },
    });
    expect(nanoVnaDescribeRequest("COM3")).toEqual({
      tool: "nanovna",
      request: { action: "describe", port: "COM3" },
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

  it("flattens a calibration step beside its port, and carries a range only when given", () => {
    expect(nanoVnaCalibrateRequest("COM3", { step: "open" })).toEqual({
      tool: "nanovna",
      request: { action: "calibrate", port: "COM3", step: "open" },
    });
    expect(nanoVnaCalibrateRequest("COM3", { step: "save", slot: 4 })).toEqual({
      tool: "nanovna",
      request: { action: "calibrate", port: "COM3", step: "save", slot: 4 },
    });
    const range = { start_hz: 1_000_000, stop_hz: 30_000_000, points: 101 };
    expect(nanoVnaCalibrateRequest("COM3", { step: "reset" }, range)).toEqual({
      tool: "nanovna",
      request: { action: "calibrate", port: "COM3", range, step: "reset" },
    });
  });

  it("unwraps only the matching NanoVNA result", () => {
    const devices: ToolResponse = {
      tool: "nanovna",
      result: { kind: "devices", devices: [], ignored_ports: ["/dev/cu.gnss"] },
    };
    expect(nanoVnaDevices(devices)).toEqual([]);
    expect(nanoVnaIgnoredPorts(devices)).toEqual(["/dev/cu.gnss"]);
    expect(nanoVnaSweep(devices)).toBeNull();
  });
});

describe("RF measurements", () => {
  it("derives matched-load measurements analytically", () => {
    const matched = { re: 0, im: 0 };
    expect(vswr(matched)).toBe(1);
    expect(impedance(matched)).toEqual({ re: 50, im: 0 });
    expect(returnLossDb(matched)).toBe(Number.POSITIVE_INFINITY);
    expect(mismatchLossDb(matched)).toBeCloseTo(0, 12);
  });

  it("derives magnitude, phase, VSWR, and impedance", () => {
    const gamma = { re: 0, im: 0.5 };
    expect(gainDb(gamma)).toBeCloseTo(-6.0206, 4);
    expect(returnLossDb(gamma)).toBeCloseTo(6.0206, 4);
    expect(phaseDeg(gamma)).toBe(90);
    expect(vswr(gamma)).toBe(3);
    expect(impedance(gamma)).toEqual({ re: 30, im: 40 });
    expect(formatImpedance(impedance(gamma))).toBe("30.0 + j40.0 Ω");
    expect(qFactor(impedance(gamma))).toBeCloseTo(40 / 30, 6);
  });

  /** A quarter of the power comes back at |Γ| = 0.5, so three quarters gets through. */
  it("prices a mismatch in forward power", () => {
    expect(mismatchLossDb({ re: 0.5, im: 0 })).toBeCloseTo(-10 * Math.log10(0.75), 6);
  });

  it("reads a reactance as the component that would produce it", () => {
    const capacitive = equivalentComponent(-1 / (2 * Math.PI * 1e6 * 1e-9), 1e6);
    expect(capacitive?.kind).toBe("capacitance");
    expect(capacitive?.value).toBeCloseTo(1e-9, 15);
    const inductive = equivalentComponent(2 * Math.PI * 1e6 * 1e-6, 1e6);
    expect(inductive?.kind).toBe("inductance");
    expect(inductive?.value).toBeCloseTo(1e-6, 12);
    expect(equivalentComponent(0, 1e6)).toBeNull();
  });

  it("finds the best match in a sweep", () => {
    const sweep = sweepOf([
      point(1, { re: 0.5, im: 0 }),
      point(2, { re: 0.1, im: 0 }),
      point(3, { re: 0.3, im: 0 }),
    ]);
    expect(lowestVswrIndex(sweep.points)).toBe(1);
  });
});

describe("phase along a sweep", () => {
  /** A line whose phase runs past ±180° must keep running, or its slope — the group delay —
   * reads as a huge spike at every wrap. */
  it("unwraps across the seam", () => {
    const points = [0, 170, -170, -10].map((degrees, index) =>
      point(index + 1, { re: 0, im: 0 }, unit(degrees)),
    );
    const phases = unwrappedPhase(points, (p) => p.s21);
    const degrees = phases.map((radians) => Math.round((radians * 180) / Math.PI));
    expect(degrees).toEqual([0, 170, 190, 350]);
  });

  it("reads a constant delay off a linear phase slope", () => {
    const delaySeconds = 5e-9;
    const points = Array.from({ length: 5 }, (_, index) => {
      const frequency = 1e6 + index * 1e5;
      const radians = -2 * Math.PI * frequency * delaySeconds;
      return point(
        frequency,
        { re: 0, im: 0 },
        {
          re: Math.cos(radians),
          im: Math.sin(radians),
        },
      );
    });
    for (const delay of groupDelays(points)) {
      expect(delay).toBeCloseTo(delaySeconds, 12);
    }
  });
});

describe("formatting", () => {
  it("scales to engineering units", () => {
    expect(formatSi(5e-9, "s", 2)).toBe("5.00 ns");
    expect(formatSi(1.2e-12, "F", 1)).toBe("1.2 pF");
    expect(formatSi(0, "s")).toBe("0 s");
    expect(formatSi(Number.NaN, "s")).toBe("—");
  });
});

function unit(degrees: number): { re: number; im: number } {
  const radians = (degrees * Math.PI) / 180;
  return { re: Math.cos(radians), im: Math.sin(radians) };
}
