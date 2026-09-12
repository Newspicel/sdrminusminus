import { describe, expect, it } from "vitest";
import type { ChannelSettings, WorkspaceDetail } from "../lib/types";
import { savedChannelsOf, withSavedChannel } from "./savedChannels";

function settings(offsetHz: number): ChannelSettings {
  return { offset_hz: offsetHz, params: { type: "nfm", settings: {} } };
}

type SavedDevice = NonNullable<NonNullable<WorkspaceDetail["state"]>["devices"]>[number];

function detail(devices: SavedDevice[]): WorkspaceDetail {
  return {
    id: 1,
    name: "desk",
    revision: 1,
    created_at: "",
    updated_at: "",
    nodes: 2,
    snapshot: {
      version: 3,
      graph: {
        nodes: [
          { id: "dev", kind: "device", position: { x: 0, y: 0 }, data: {} },
          { id: "voice", kind: "channel", position: { x: 0, y: 0 }, data: { channel_type: "nfm" } },
        ],
        edges: [{ from: { node: "dev", port: "iq" }, to: { node: "voice", port: "iq" } }],
      },
    },
    state: { version: 1, devices },
  } as unknown as WorkspaceDetail;
}

describe("savedChannelsOf", () => {
  it("keys every held channel by its node", () => {
    const held = savedChannelsOf(
      detail([
        { node: "dev", settings: {}, channels: [{ node: "voice", settings: settings(500) }] },
      ]),
    );
    expect(held.get("voice")?.offset_hz).toBe(500);
  });

  it("is empty for a workspace nothing has been held for", () => {
    expect(savedChannelsOf(detail([])).size).toBe(0);
    expect(savedChannelsOf(null).size).toBe(0);
  });
});

describe("withSavedChannel", () => {
  it("replaces what a node was already held on", () => {
    const before = detail([
      { node: "dev", settings: {}, channels: [{ node: "voice", settings: settings(500) }] },
    ]);
    const after = withSavedChannel(before, "voice", settings(1_500));
    expect(savedChannelsOf(after).get("voice")?.offset_hz).toBe(1_500);
    expect(savedChannelsOf(before).get("voice")?.offset_hz).toBe(500);
  });

  it("hangs a first edit off the device node the wire names", () => {
    const before = detail([{ node: "dev", settings: {}, channels: [] }]);
    const after = withSavedChannel(before, "voice", settings(2_500));
    expect(savedChannelsOf(after).get("voice")?.offset_hz).toBe(2_500);
  });

  it("leaves a node no radio feeds alone", () => {
    const before = detail([{ node: "other", settings: {}, channels: [] }]);
    expect(withSavedChannel(before, "voice", settings(2_500))).toBe(before);
  });
});
