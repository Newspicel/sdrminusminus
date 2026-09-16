import { describe, expect, it } from "vitest";
import type { ChannelDescriptor, DeviceSet } from "./types";
import { radioPullFor, sameMode } from "./useTuner";

function set(over: Partial<DeviceSet["settings"]> = {}): DeviceSet {
  return {
    id: 1,
    device: { driver: "rtlsdr", key: "0", label: "RTL-SDR" },
    capabilities: {
      freq_ranges: [],
      sample_rates: [],
      gains: [],
      antennas: [],
      bandwidths: [],
      duplex: "rx_only",
    },
    settings: { center_hz: 145_000_000, sample_rate: 2_000_000, ...over },
    status: "running",
    channels: [],
    overruns: 0,
  };
}

const NFM = { type_id: "nfm", bandwidth_hz: 12_500 } as ChannelDescriptor;

describe("radioPullFor", () => {
  it("leaves the radio where it is when it already hears the frequency", () => {
    expect(radioPullFor(set(), 0, NFM, 145_500_000)).toBeNull();
  });

  it("pulls the radio over a frequency outside its window", () => {
    expect(radioPullFor(set(), 0, NFM, 433_500_000)).toEqual({ center_hz: 433_500_000 });
  });

  it("leaves a radio alone that has no window yet", () => {
    expect(radioPullFor(set({ sample_rate: undefined }), 0, NFM, 433_500_000)).toBeNull();
  });
});

describe("sameMode", () => {
  it("treats a missing mode as nothing to say", () => {
    expect(sameMode(null, "nfm")).toBe(true);
    expect(sameMode("", "nfm")).toBe(true);
  });

  it("matches a decoder ignoring case", () => {
    expect(sameMode("NFM", "nfm")).toBe(true);
    expect(sameMode("am", "nfm")).toBe(false);
  });

  it("never matches a device, which runs no mode", () => {
    expect(sameMode("nfm", null)).toBe(false);
  });
});
