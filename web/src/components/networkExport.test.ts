import { describe, expect, it } from "vitest";
import type { NetworkExportStatus } from "../lib/types";
import {
  channelExportSource,
  deriveNetworkExportControl,
  deviceExportSource,
} from "./networkExport";

const status: NetworkExportStatus = {
  node: "net-a",
  stream: 0,
  settings: { transport: "udp", format: "cf32_le", address: "127.0.0.1:7355" },
  sample_rate: 2_400_000,
  center_hz: 100_000_000,
  samples: 10,
  bytes: 80,
  packets: 1,
  overruns: 0,
};

describe("deriveNetworkExportControl", () => {
  it("distinguishes the owner from another sink on the same radio", () => {
    const running = deviceExportSource({ status: "running", network_export: status });
    expect(deriveNetworkExportControl(running, "net-a")).toEqual({ kind: "active", status });
    expect(deriveNetworkExportControl(running, "net-b")).toEqual({
      kind: "busy",
      owner: "net-a",
    });
  });

  it("only offers start on a running, unclaimed radio", () => {
    expect(deriveNetworkExportControl(deviceExportSource({ status: "running" }), "net-a")).toEqual({
      kind: "ready",
    });
    expect(deriveNetworkExportControl(deviceExportSource({ status: "error" }), "net-a")).toEqual({
      kind: "unavailable",
    });
    expect(deriveNetworkExportControl(deviceExportSource(null), "net-a")).toEqual({
      kind: "unavailable",
    });
  });

  it("reads a channel's own export, not the radio's", () => {
    const set = { status: "running", network_export: status };
    const channel = { network_export: { ...status, node: "bb-a", sample_rate: 48_000 } };
    expect(deriveNetworkExportControl(channelExportSource(set, channel), "bb-a")).toEqual({
      kind: "active",
      status: channel.network_export,
    });
    expect(deriveNetworkExportControl(channelExportSource(set, {}), "bb-a")).toEqual({
      kind: "ready",
    });
    expect(deriveNetworkExportControl(channelExportSource(set, null), "bb-a")).toEqual({
      kind: "unavailable",
    });
  });
});
