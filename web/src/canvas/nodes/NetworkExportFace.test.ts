import { describe, expect, it, vi } from "vitest";
import {
  networkExportControlsLocked,
  networkExportMutationOptions,
} from "../../components/networkExport";
import type { NetworkExportSettings, NetworkExportStatus } from "../../lib/types";

const settings: NetworkExportSettings = {
  transport: "tcp",
  format: "ci16_le",
  address: "analysis.local:7355",
};

const status: NetworkExportStatus = {
  node: "net-a",
  stream: 2,
  settings,
  sample_rate: 2_400_000,
  center_hz: 100_000_000,
  samples: 10,
  bytes: 40,
  packets: 1,
  overruns: 0,
};

function requests() {
  return {
    device: vi.fn(async () => status),
    channel: vi.fn(async () => status),
  };
}

describe("NetworkExportFace lifecycle", () => {
  it("sends start and stop with the bound stream and current settings", async () => {
    const request = requests();
    const options = networkExportMutationOptions(
      { kind: "device", deviceSet: 7, stream: 2 },
      "net-a",
      settings,
      request,
    );

    await options.mutationFn("start");
    await options.mutationFn("stop");

    expect(request.device).toHaveBeenNthCalledWith(1, 7, "start", "net-a", 2, settings);
    expect(request.device).toHaveBeenNthCalledWith(2, 7, "stop", "net-a", 2, settings);
    expect(request.channel).not.toHaveBeenCalled();
  });

  it("sends a channel's baseband export to the channel route", async () => {
    const request = requests();
    const options = networkExportMutationOptions(
      { kind: "channel", deviceSet: 7, channel: 3 },
      "net-a",
      settings,
      request,
    );

    await options.mutationFn("start");

    expect(request.channel).toHaveBeenCalledWith(7, 3, "start", "net-a", settings);
    expect(request.device).not.toHaveBeenCalled();
  });

  it("locks settings during a request and throughout an active export", () => {
    expect(networkExportControlsLocked({ kind: "ready" }, false)).toBe(false);
    expect(networkExportControlsLocked({ kind: "ready" }, true)).toBe(true);
    expect(networkExportControlsLocked({ kind: "active", status }, false)).toBe(true);
  });

  it("routes request failures to the toast callback", () => {
    const notify = vi.fn();
    const options = networkExportMutationOptions(null, "net-a", settings, undefined, notify);
    options.onError(new Error("destination refused"));
    expect(notify).toHaveBeenCalledWith("destination refused");
  });

  it("refuses an action when no live source is bound", async () => {
    const request = requests();
    const options = networkExportMutationOptions(null, "net-a", settings, request);
    await expect(options.mutationFn("start")).rejects.toThrow("Wire a running device's IQ");
    expect(request.device).not.toHaveBeenCalled();
    expect(request.channel).not.toHaveBeenCalled();
  });
});
