import { describe, expect, it } from "vitest";
import { exportFilename, sweepCsv, touchstoneS1p, touchstoneS2p } from "./export";
import { point, sweepOf } from "./testdata";

const SWEEP = sweepOf([
  point(1_000_000, { re: 0, im: 0.5 }, { re: 0.5, im: 0 }),
  point(2_000_000, { re: 0.25, im: -0.25 }, { re: 0.9, im: 0.1 }),
]);

describe("Touchstone", () => {
  it("writes a two-port file with the option line the reader needs", () => {
    const lines = touchstoneS2p(SWEEP, "ri").trim().split("\n");
    expect(lines).toContain("# Hz S RI R 50");
    const first = lines.at(-2);
    expect(first).toBe(
      "1000000 0.000000000 0.500000000 0.500000000 0.000000000 0.000000000 0.000000000 0.000000000 0.000000000",
    );
  });

  it("says in the file that S12 and S22 were never measured", () => {
    expect(touchstoneS2p(SWEEP, "ri")).toContain(
      "! S12 and S22 are not measured by this instrument and are written as zero.",
    );
  });

  it("writes magnitude/angle and dB/angle on request", () => {
    const ma = touchstoneS2p(SWEEP, "ma").trim().split("\n").at(-2);
    expect(ma?.split(" ").slice(1, 3)).toEqual(["0.500000000", "90.000000"]);
    const db = touchstoneS2p(SWEEP, "db").trim().split("\n").at(-2);
    expect(db?.split(" ")[1]).toBe("-6.020600");
  });

  it("writes a one-port file with only the reflection", () => {
    const lines = touchstoneS1p(SWEEP, "ri").trim().split("\n");
    expect(lines).toContain("# Hz S RI R 50");
    expect(lines.at(-1)).toBe("2000000 0.250000000 -0.250000000");
  });

  it("carries the instrument and its calibration in the header", () => {
    const header = touchstoneS2p(SWEEP, "ri", { recordedAt: "2026-08-15T18:00:00Z" });
    expect(header).toContain("! Recorded 2026-08-15T18:00:00Z");
    expect(header).toContain("! Instrument NanoVNA-H 4 firmware 1.2.46 on /dev/cu.usbmodem4001");
    expect(header).toContain("! Calibration load isoln Es Er Et cal'ed");
    expect(header).toContain("! IF bandwidth 1000 Hz");
  });
});

describe("CSV", () => {
  it("heads every derived column and writes one row per point", () => {
    const rows = sweepCsv(SWEEP).trim().split("\n");
    expect(rows[0]?.split(",")).toContain("group_delay_s");
    expect(rows[0]?.split(",")).toContain("series_inductance_h");
    expect(rows).toHaveLength(3);
    const first = rows[1]?.split(",") ?? [];
    expect(first[0]).toBe("1000000");
    expect(Number(first[5])).toBeCloseTo(3, 9);
  });

  it("writes an unmeasurable cell as empty and an infinite one as inf", () => {
    const matched = sweepCsv(sweepOf([point(1_000_000, { re: 0, im: 0 })]));
    const cells = matched.trim().split("\n")[1]?.split(",") ?? [];
    const columns = matched.split("\n")[0]?.split(",") ?? [];
    expect(cells[columns.indexOf("return_loss_db")]).toBe("inf");
    expect(cells[columns.indexOf("series_capacitance_f")]).toBe("");
  });
});

describe("filenames", () => {
  it("names the instrument and the range it swept", () => {
    expect(exportFilename(SWEEP, "s2p")).toBe("nanovna-h-4-1000khz-to-2000khz.s2p");
  });
});
