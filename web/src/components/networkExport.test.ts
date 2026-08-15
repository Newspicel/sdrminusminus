import { describe, expect, it } from "vitest";
import type { NetworkExportStatus } from "../lib/types";
import { deriveNetworkExportControl } from "./networkExport";

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
    expect(
      deriveNetworkExportControl({ status: "running", network_export: status }, "net-a"),
    ).toEqual({ kind: "active", status });
    expect(
      deriveNetworkExportControl({ status: "running", network_export: status }, "net-b"),
    ).toEqual({ kind: "busy", owner: "net-a" });
  });

  it("only offers start on a running, unclaimed radio", () => {
    expect(deriveNetworkExportControl({ status: "running" }, "net-a")).toEqual({
      kind: "ready",
    });
    expect(deriveNetworkExportControl({ status: "error" }, "net-a")).toEqual({
      kind: "unavailable",
    });
    expect(deriveNetworkExportControl(null, "net-a")).toEqual({ kind: "unavailable" });
  });
});
