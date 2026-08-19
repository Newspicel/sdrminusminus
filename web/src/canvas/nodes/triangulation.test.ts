import { describe, expect, it } from "vitest";
import type { DfStation } from "../../lib/types";
import { spreadLabel, stationAge } from "./triangulation";

function station(last_seen: string): DfStation {
  return { station_id: "north", lat: 51.5, lon: 7.0, bearings: 3, last_seen };
}

describe("spreadLabel", () => {
  it("reads the ellipse in units a driver can judge", () => {
    expect(
      spreadLabel({
        lat: 51.5,
        lon: 7.0,
        ellipse_major_m: 2_400,
        ellipse_minor_m: 180,
        ellipse_bearing_deg: 45,
        converged: false,
        samples: 6,
      }),
    ).toBe("2.4 km × 180 m");
    expect(spreadLabel(null)).toBe("—");
  });
});

describe("stationAge", () => {
  const now = Date.parse("2026-01-01T00:10:00Z");

  it("says how long ago a finder last reported", () => {
    expect(stationAge(station("2026-01-01T00:09:58Z"), now)).toBe("just now");
    expect(stationAge(station("2026-01-01T00:09:30Z"), now)).toBe("30s ago");
    expect(stationAge(station("2026-01-01T00:05:00Z"), now)).toBe("5m ago");
  });

  it("does not pretend to know an unreadable time", () => {
    expect(stationAge(station("who knows"), now)).toBe("just now");
  });
});
