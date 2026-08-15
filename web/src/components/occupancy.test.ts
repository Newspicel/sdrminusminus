import { describe, expect, it } from "vitest";
import type { OccupancyBucket, OccupancyReport } from "../lib/types";
import {
  busiestHour,
  dutyAlpha,
  formatBucketHz,
  formatDuty,
  formatHour,
  hasOccupancy,
  MAX_ROWS,
  occupancyRows,
} from "./occupancy";

function bucket(freqHz: number, duty: number, byHour: number[] = []): OccupancyBucket {
  return {
    freq_hz: freqHz,
    duty,
    samples: 100,
    by_hour: Array.from({ length: 24 }, (_, hour) => byHour[hour] ?? 0),
    last_seen: "2026-08-15T05:00:00Z",
  };
}

function report(buckets: OccupancyBucket[]): OccupancyReport {
  return { bucket_hz: 12_500, since: "2026-08-15T00:00:00Z", buckets };
}

describe("occupancyRows", () => {
  it("keeps the server's busiest-first order by default", () => {
    const rows = occupancyRows(report([bucket(145.5e6, 0.4), bucket(144.8e6, 0.1)]), "busiest");
    expect(rows.map((row) => row.freq_hz)).toEqual([145.5e6, 144.8e6]);
  });

  it("resorts by frequency when asked", () => {
    const rows = occupancyRows(report([bucket(145.5e6, 0.4), bucket(144.8e6, 0.1)]), "frequency");
    expect(rows.map((row) => row.freq_hz)).toEqual([144.8e6, 145.5e6]);
  });

  it("filters on the printed frequency", () => {
    const rows = occupancyRows(
      report([bucket(145.5e6, 0.4), bucket(144.8e6, 0.1)]),
      "busiest",
      "145.5",
    );
    expect(rows.map((row) => row.freq_hz)).toEqual([145.5e6]);
  });

  it("caps the rows it hands back", () => {
    const many = Array.from({ length: MAX_ROWS + 20 }, (_, i) => bucket(100e6 + i * 12_500, 0.5));
    expect(occupancyRows(report(many), "busiest")).toHaveLength(MAX_ROWS);
    expect(occupancyRows(report(many), "busiest", "", 5)).toHaveLength(5);
  });

  it("has nothing to draw before a report has arrived", () => {
    expect(occupancyRows(null, "busiest")).toEqual([]);
    expect(hasOccupancy(null)).toBe(false);
    expect(hasOccupancy(report([]))).toBe(false);
    expect(hasOccupancy(report([bucket(1e6, 0)]))).toBe(true);
  });
});

describe("dutyAlpha", () => {
  it("lifts the low end without claiming a quiet frequency is busy", () => {
    // 4% duty would be all but invisible drawn linearly.
    expect(dutyAlpha(0.04)).toBeCloseTo(0.2, 6);
    expect(dutyAlpha(0.04)).toBeGreaterThan(0.04);
    expect(dutyAlpha(1)).toBe(1);
  });

  it("draws nothing at all where nothing was observed", () => {
    expect(dutyAlpha(0)).toBe(0);
    expect(dutyAlpha(-1)).toBe(0);
    expect(dutyAlpha(Number.NaN)).toBe(0);
  });

  it("rises with duty and clamps past full", () => {
    expect(dutyAlpha(0.5)).toBeGreaterThan(dutyAlpha(0.2));
    expect(dutyAlpha(4)).toBe(1);
  });
});

describe("busiestHour", () => {
  it("finds the hour a frequency is most used", () => {
    const hours = Array.from({ length: 24 }, (_, hour) => (hour === 7 ? 0.9 : 0.1));
    expect(busiestHour(bucket(145.5e6, 0.2, hours))).toBe(7);
  });

  it("says nothing about a frequency that was never busy", () => {
    expect(busiestHour(bucket(145.5e6, 0))).toBeNull();
  });
});

describe("formatting", () => {
  it("prints a bucket to the resolution it actually has", () => {
    expect(formatBucketHz(145_506_300)).toBe("145.5063 MHz");
    expect(formatBucketHz(145_500_000)).toBe("145.5000 MHz");
  });

  it("prints duty as a percentage, and absence as a dash", () => {
    expect(formatDuty(0.123)).toBe("12%");
    expect(formatDuty(1)).toBe("100%");
    expect(formatDuty(0)).toBe("—");
    expect(formatDuty(Number.NaN)).toBe("—");
  });

  it("pads the hour labels so the columns line up", () => {
    expect(formatHour(0)).toBe("00:00");
    expect(formatHour(7)).toBe("07:00");
    expect(formatHour(23)).toBe("23:00");
  });
});
