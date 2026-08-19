import { describe, expect, it } from "vitest";
import type { RadarDetection } from "../lib/types";
import { trackLabel } from "./radarTrack";

function hit(over: Partial<RadarDetection> = {}): RadarDetection {
  return { range_bin: 40, range_km: 12.0, doppler_hz: 200, snr_db: 14, track_id: 3, ...over };
}

describe("trackLabel", () => {
  it("says how far out a target is and which way it is moving", () => {
    expect(trackLabel(hit())).toBe("03  12.0 km  200 Hz closing");
    expect(trackLabel(hit({ doppler_hz: -80 }))).toBe("03  12.0 km  80 Hz opening");
  });

  it("has no name for an echo the tracker has not decided about", () => {
    expect(trackLabel(hit({ track_id: null })).startsWith("—")).toBe(true);
  });
});
