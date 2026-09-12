import { describe, expect, it } from "vitest";
import type { ChannelSettings, WorkspaceDetail } from "../lib/types";
import { savedChannelsOf, withSavedChannel } from "./savedChannels";

function settings(frequencyHz: number): ChannelSettings {
  return { frequency_hz: frequencyHz, params: { type: "nfm", settings: {} } };
}

type SavedChannel = NonNullable<NonNullable<WorkspaceDetail["state"]>["channels"]>[number];

function detail(channels: SavedChannel[]): WorkspaceDetail {
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
    state: { version: 2, devices: [], channels },
  } as unknown as WorkspaceDetail;
}

describe("savedChannelsOf", () => {
  it("keys every held channel by its node", () => {
    const held = savedChannelsOf(detail([{ node: "voice", settings: settings(145_500_000) }]));
    expect(held.get("voice")?.frequency_hz).toBe(145_500_000);
  });

  it("is empty for a workspace nothing has been held for", () => {
    expect(savedChannelsOf(detail([])).size).toBe(0);
    expect(savedChannelsOf(null).size).toBe(0);
  });
});

describe("withSavedChannel", () => {
  it("replaces what a node was already held on", () => {
    const before = detail([{ node: "voice", settings: settings(145_500_000) }]);
    const after = withSavedChannel(before, "voice", settings(433_920_000));
    expect(savedChannelsOf(after).get("voice")?.frequency_hz).toBe(433_920_000);
    expect(savedChannelsOf(before).get("voice")?.frequency_hz).toBe(145_500_000);
  });

  it("holds a node no radio feeds", () => {
    const before = detail([]);
    const after = withSavedChannel(before, "voice", settings(433_920_000));
    expect(savedChannelsOf(after).get("voice")?.frequency_hz).toBe(433_920_000);
  });
});
