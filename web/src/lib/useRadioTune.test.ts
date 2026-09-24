import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DeviceSet, DeviceSettings } from "./types";
import { takeOver } from "./useRadioTune";

const pushNote = vi.hoisted(() => vi.fn());
vi.mock("./toasts", () => ({ pushNote }));

function radio(settings: DeviceSettings): DeviceSet {
  return {
    id: 3,
    device: { driver: "virtual", key: "siggen", label: "Signal Generator" },
    capabilities: {
      freq_ranges: [],
      sample_rates: [],
      gains: [],
      antennas: [],
      bandwidths: [],
      extra: [],
      duplex: "rx_only",
    },
    settings,
    status: "running",
    channels: [],
    overruns: 0,
  };
}

describe("takeOver", () => {
  beforeEach(() => pushNote.mockReset());

  it("tunes by hand and offers the way back to Auto", () => {
    const applyPatch = vi.fn();
    const set = radio({ tuning: "auto" });
    takeOver(applyPatch, () => undefined, { set, tunes: 0 }, 100_000_000);
    expect(applyPatch).toHaveBeenCalledWith(3, { center_hz: 100_000_000, tuning: "manual" });
    expect(pushNote).toHaveBeenCalledOnce();
    pushNote.mock.calls[0]?.[1].run();
    expect(applyPatch).toHaveBeenLastCalledWith(3, { tuning: "auto" });
  });

  it("stays quiet when the radio is already manual", () => {
    const applyPatch = vi.fn();
    const set = radio({ tuning: "auto" });
    takeOver(applyPatch, () => ({ tuning: "manual" }), { set, tunes: 0 }, 100_000_000);
    expect(pushNote).not.toHaveBeenCalled();
  });
});
