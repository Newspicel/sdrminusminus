import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ChannelInfo, DeviceSet, PatchGraph, PatchNode } from "../lib/types";
import type { Workspace } from "./context";
import { closeEngineObjects, releaseRadio } from "./remove";

const api = vi.hoisted(() => ({
  controlTimeMachine: vi.fn(async () => {}),
  deleteChannel: vi.fn(async () => {}),
  deleteDeviceSet: vi.fn(async () => {}),
  networkExportChannel: vi.fn(async () => {}),
  networkExportDeviceSet: vi.fn(async () => {}),
}));
vi.mock("../lib/api", () => api);

function node(id: string, body: Partial<PatchNode> & Pick<PatchNode, "kind">): PatchNode {
  return { id, position: { x: 0, y: 0 }, ...body } as PatchNode;
}

const graph: PatchGraph = {
  nodes: [
    node("dev", { kind: "device", data: {} }),
    node("high", { kind: "channel", data: { channel_type: "nfm" } }),
  ],
  edges: [{ from: { node: "dev", port: "iq3" }, to: { node: "high", port: "iq" } }],
};

const channelOnLane2: ChannelInfo = {
  id: 7,
  stream: 2,
  settings: { offset_hz: 0, params: { type: "nfm", settings: {} } as never },
};

const workspace = {
  graph,
  devices: new Map([["dev", { id: 4 } as DeviceSet]]),
  channels: new Map([["high", channelOnLane2]]),
} as unknown as Workspace;

const networkSettings = {
  transport: "udp" as const,
  format: "ci16_le" as const,
  address: "127.0.0.1:7355",
};
const networkGraph: PatchGraph = {
  nodes: [
    node("dev", { kind: "device", data: {} }),
    node("net-owned", { kind: "network_export", data: networkSettings }),
    node("net-other", { kind: "network_export", data: networkSettings }),
  ],
  edges: [
    { from: { node: "dev", port: "iq2" }, to: { node: "net-owned", port: "iq" } },
    { from: { node: "dev", port: "iq3" }, to: { node: "net-other", port: "iq" } },
  ],
};
const networkWorkspace = {
  ...workspace,
  graph: networkGraph,
  devices: new Map([
    [
      "dev",
      {
        id: 4,
        network_export: {
          node: "net-owned",
          stream: 1,
          settings: networkSettings,
          sample_rate: 2_048_000,
          center_hz: 100_000_000,
          samples: 128,
          bytes: 512,
          packets: 1,
          overruns: 0,
        },
      } as DeviceSet,
    ],
  ]),
} as unknown as Workspace;

const basebandChannel: ChannelInfo = {
  ...channelOnLane2,
  id: 9,
  stream: 0,
  network_export: {
    node: "net-baseband",
    stream: 0,
    settings: networkSettings,
    sample_rate: 48_000,
    center_hz: 100_000_000,
    samples: 64,
    bytes: 512,
    packets: 1,
    overruns: 0,
  },
};
const basebandWorkspace = {
  ...workspace,
  graph: {
    nodes: [
      node("dev", { kind: "device", data: {} }),
      node("ch", { kind: "channel", data: { channel_type: "nfm" } }),
      node("net-baseband", { kind: "network_export", data: networkSettings }),
      node("history", { kind: "time_machine", data: { history_seconds: 10 } }),
    ],
    edges: [
      { from: { node: "dev", port: "iq" }, to: { node: "ch", port: "iq" } },
      { from: { node: "ch", port: "baseband" }, to: { node: "net-baseband", port: "baseband" } },
      { from: { node: "dev", port: "iq" }, to: { node: "history", port: "iq" } },
    ],
  },
  devices: new Map([
    [
      "dev",
      {
        id: 4,
        time_machine: { node: "history", stream: 0 },
      } as unknown as DeviceSet,
    ],
  ]),
  channels: new Map([["ch", basebandChannel]]),
} as unknown as Workspace;

describe("closeEngineObjects", () => {
  beforeEach(() => {
    api.controlTimeMachine.mockClear();
    api.deleteChannel.mockClear();
    api.deleteDeviceSet.mockClear();
    api.networkExportChannel.mockClear();
    api.networkExportDeviceSet.mockClear();
  });

  it("stops a baseband export on the channel it belongs to", async () => {
    await closeEngineObjects(basebandWorkspace, ["net-baseband"]);
    expect(api.networkExportChannel).toHaveBeenCalledWith(
      4,
      9,
      "stop",
      "net-baseband",
      networkSettings,
    );
    expect(api.networkExportDeviceSet).not.toHaveBeenCalled();
  });

  it("disarms the time machine a removed sink is holding", async () => {
    await closeEngineObjects(basebandWorkspace, ["history"]);
    expect(api.controlTimeMachine).toHaveBeenCalledWith(4, "disarm", "history", 0, {
      history_seconds: 10,
    });
  });

  it("deletes the engine channel behind a node wired past stream 0", async () => {
    await closeEngineObjects(workspace, ["high"]);
    expect(api.deleteChannel).toHaveBeenCalledWith(4, 7);
    expect(api.deleteDeviceSet).not.toHaveBeenCalled();
  });

  it("closes the device set behind a device node", async () => {
    await closeEngineObjects(workspace, ["dev"]);
    expect(api.deleteDeviceSet).toHaveBeenCalledWith(4);
    expect(api.deleteChannel).not.toHaveBeenCalled();
  });

  it("stops only the network export owned by a removed sink", async () => {
    await closeEngineObjects(networkWorkspace, ["net-other"]);
    expect(api.networkExportDeviceSet).not.toHaveBeenCalled();

    await closeEngineObjects(networkWorkspace, ["net-owned"]);
    expect(api.networkExportDeviceSet).toHaveBeenCalledTimes(1);
    expect(api.networkExportDeviceSet).toHaveBeenCalledWith(
      4,
      "stop",
      "net-owned",
      1,
      networkSettings,
    );
  });
});

describe("releaseRadio", () => {
  beforeEach(() => {
    api.deleteChannel.mockClear();
    api.deleteDeviceSet.mockClear();
    api.networkExportDeviceSet.mockClear();
  });

  it("closes the radio before unbinding the node", async () => {
    const order: string[] = [];
    api.deleteDeviceSet.mockImplementation(async () => {
      order.push("closed");
    });

    await releaseRadio(workspace, "dev", () => order.push("unbound"));

    expect(order).toEqual(["closed", "unbound"]);
  });

  it("leaves the node bound when the radio refuses to close", async () => {
    api.deleteDeviceSet.mockRejectedValue(new Error("device busy"));
    let unbound = false;

    await expect(releaseRadio(workspace, "dev", () => (unbound = true))).rejects.toThrow(
      "device busy",
    );
    expect(unbound).toBe(false);
  });

  it("unbinds a node whose radio is not attached, with nothing to close", async () => {
    let unbound = false;
    const detached = { ...workspace, devices: new Map() } as unknown as Workspace;

    await releaseRadio(detached, "dev", () => (unbound = true));

    expect(unbound).toBe(true);
    expect(api.deleteDeviceSet).not.toHaveBeenCalled();
  });
});
