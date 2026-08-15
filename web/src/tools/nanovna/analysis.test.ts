import { describe, expect, it } from "vitest";
import { analyse, bandSpan, readouts } from "./analysis";
import { point, resonantSweep, sweepOf } from "./testdata";

describe("readouts", () => {
  it("derives every quantity one frequency carries", () => {
    const rows = readouts([point(1_000_000, { re: 0, im: 0.5 }, { re: 0.5, im: 0 })]);
    const row = rows[0];
    expect(row).toBeDefined();
    expect(row?.vswr).toBe(3);
    expect(row?.impedance).toEqual({ re: 30, im: 40 });
    expect(row?.impedanceMagnitude).toBeCloseTo(50, 9);
    expect(row?.s21Db).toBeCloseTo(-6.0206, 4);
    expect(row?.insertionLossDb).toBeCloseTo(6.0206, 4);
    expect(row?.component?.kind).toBe("inductance");
  });
});

describe("sweep analysis", () => {
  const analysis = analyse(resonantSweep().points);

  it("finds the resonance the load was built around", () => {
    expect(analysis.resonance?.frequencyHz).toBeGreaterThan(14_080_000);
    expect(analysis.resonance?.frequencyHz).toBeLessThan(14_120_000);
    expect(analysis.resonance?.vswr).toBeLessThan(1.01);
  });

  it("nests the VSWR bands", () => {
    const spans = analysis.vswrBands.map(({ band }) => band?.spanHz ?? 0);
    expect(spans[0]).toBeGreaterThan(0);
    expect(spans[0]).toBeLessThan(spans[1] ?? 0);
    expect(spans[1]).toBeLessThan(spans[2] ?? 0);
  });

  it("reports the loaded Q of the band it measured", () => {
    const band = analysis.vswrBands.find((entry) => entry.limit === 2)?.band;
    expect(band?.truncated).toBe(false);
    expect(band?.q).toBeGreaterThan(1);
  });

  it("refuses to call the noise floor a passband", () => {
    const quiet = analyse(
      Array.from({ length: 21 }, (_, index) =>
        point(1_000_000 + index * 100_000, { re: 0.99, im: 0 }, { re: 1e-5, im: 1e-5 }),
      ),
    );
    expect(quiet.transmitting).toBe(false);
    expect(quiet.peak).toBeNull();
    expect(quiet.transmissionBand).toBeNull();
  });

  it("describes an empty sweep without inventing one", () => {
    const empty = analyse([]);
    expect(empty.count).toBe(0);
    expect(empty.resonance).toBeNull();
    expect(empty.spanHz).toBe(0);
  });
});

describe("bandSpan", () => {
  const frequencies = [1, 2, 3, 4, 5];

  it("interpolates the edges between the samples either side of the limit", () => {
    const band = bandSpan(frequencies, [4, 2, 0, 2, 4], 2, 1, true);
    expect(band?.startHz).toBeCloseTo(2.5, 9);
    expect(band?.stopHz).toBeCloseTo(3.5, 9);
    expect(band?.spanHz).toBeCloseTo(1, 9);
    expect(band?.truncated).toBe(false);
  });

  it("marks a band that reaches the edge of the sweep", () => {
    const band = bandSpan(frequencies, [0, 0, 0, 0, 0], 2, 1, true);
    expect(band?.truncated).toBe(true);
    expect(band?.startHz).toBe(1);
    expect(band?.stopHz).toBe(5);
  });

  it("finds nothing when the reference point is already outside the limit", () => {
    expect(bandSpan(frequencies, [4, 4, 4, 4, 4], 2, 1, true)).toBeNull();
  });

  it("walks upward for a limit a trace has to stay above", () => {
    const band = bandSpan(frequencies, [0, 0, 10, 0, 0], 2, 5, false);
    expect(band?.startHz).toBeCloseTo(2.5, 9);
    expect(band?.stopHz).toBeCloseTo(3.5, 9);
  });
});

describe("group delay across a sweep", () => {
  it("leaves the ends defined", () => {
    const rows = readouts(sweepOf([point(1, { re: 0, im: 0 }), point(2, { re: 0, im: 0 })]).points);
    expect(rows.every((row) => Number.isFinite(row.groupDelayS))).toBe(true);
  });
});
