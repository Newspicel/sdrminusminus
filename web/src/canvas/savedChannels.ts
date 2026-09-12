import type { ChannelSettings, WorkspaceDetail } from "../lib/types";
import { deviceNodeOf } from "./binding";

export function savedChannelsOf(
  detail: WorkspaceDetail | null,
): ReadonlyMap<string, ChannelSettings> {
  const held = new Map<string, ChannelSettings>();
  for (const device of detail?.state?.devices ?? []) {
    for (const channel of device.channels ?? []) {
      held.set(channel.node, channel.settings);
    }
  }
  return held;
}

/// Mirrors what the server will hold, so a control does not snap back while the write is in air.
export function withSavedChannel(
  detail: WorkspaceDetail,
  node: string,
  settings: ChannelSettings,
): WorkspaceDetail {
  const state = detail.state ?? { version: 1, devices: [] };
  const devices = state.devices ?? [];
  const holder =
    devices.find((device) => (device.channels ?? []).some((held) => held.node === node)) ??
    devices.find((device) => device.node === deviceNodeOf(detail.snapshot.graph, node));
  if (holder === undefined) {
    return detail;
  }
  const channels = holder.channels ?? [];
  return {
    ...detail,
    state: {
      ...state,
      devices: devices.map((device) =>
        device === holder
          ? {
              ...device,
              channels: channels.some((held) => held.node === node)
                ? channels.map((held) => (held.node === node ? { ...held, settings } : held))
                : [...channels, { node, settings }],
            }
          : device,
      ),
    },
  };
}
