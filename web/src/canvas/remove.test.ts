// Removal must close the engine objects behind the nodes it takes, whichever lane feeds them:
// resolved as the bare `iq`, a channel wired to `iq3` would leak its engine channel — the node
// disappears from the patch while the demod keeps running.
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ChannelInfo, DeviceSet, PatchGraph, PatchNode } from "../lib/types";
import type { Workspace } from "./context";
import { closeEngineObjects } from "./remove";

const api = vi.hoisted(() => ({
  deleteChannel: vi.fn(async () => {}),
  deleteDeviceSet: vi.fn(async () => {}),
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

// Only the three fields `closeEngineObjects` reads; the rest of the workspace is React state
// with no bearing on removal.
const workspace = {
  graph,
  devices: new Map([["dev", { id: 4 } as DeviceSet]]),
  channels: new Map([["high", channelOnLane2]]),
} as unknown as Workspace;

describe("closeEngineObjects", () => {
  beforeEach(() => {
    api.deleteChannel.mockClear();
    api.deleteDeviceSet.mockClear();
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
});
