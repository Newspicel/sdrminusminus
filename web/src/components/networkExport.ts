import { networkExportChannel, networkExportDeviceSet } from "../lib/api";
import { pushToast } from "../lib/toasts";
import type { NetworkExportAction, NetworkExportSettings, NetworkExportStatus } from "../lib/types";

export type NetworkExportControl =
  | { kind: "unavailable" }
  | { kind: "ready" }
  | { kind: "active"; status: NetworkExportStatus }
  | { kind: "busy"; owner: string };

export type NetworkExportTarget =
  | { kind: "device"; deviceSet: number; stream: number }
  | { kind: "channel"; deviceSet: number; channel: number };

export interface NetworkExportSource {
  running: boolean;
  active?: NetworkExportStatus | null;
}

export interface NetworkExportRequests {
  device: typeof networkExportDeviceSet;
  channel: typeof networkExportChannel;
}

const REQUESTS: NetworkExportRequests = {
  device: networkExportDeviceSet,
  channel: networkExportChannel,
};

export function deviceExportSource(
  set: { status: string; network_export?: NetworkExportStatus | null } | null,
): NetworkExportSource | null {
  return set === null ? null : { running: set.status === "running", active: set.network_export };
}

export function channelExportSource(
  set: { status: string } | null,
  channel: { network_export?: NetworkExportStatus | null } | null,
): NetworkExportSource | null {
  return set === null || channel === null
    ? null
    : { running: set.status === "running", active: channel.network_export };
}

export function deriveNetworkExportControl(
  source: NetworkExportSource | null,
  node: string,
): NetworkExportControl {
  if (source === null || !source.running) {
    return { kind: "unavailable" };
  }
  const active = source.active;
  if (active == null) {
    return { kind: "ready" };
  }
  return active.node === node
    ? { kind: "active", status: active }
    : { kind: "busy", owner: active.node };
}

export function networkExportMutationOptions(
  target: NetworkExportTarget | null,
  node: string,
  settings: NetworkExportSettings,
  requests: NetworkExportRequests = REQUESTS,
  notify: (message: string) => void = pushToast,
) {
  return {
    mutationFn: (action: NetworkExportAction) => {
      if (target === null) {
        return Promise.reject(
          new Error("Wire a running device's IQ or a channel's baseband into this sink first."),
        );
      }
      return target.kind === "device"
        ? requests.device(target.deviceSet, action, node, target.stream, settings)
        : requests.channel(target.deviceSet, target.channel, action, node, settings);
    },
    onError: (error: Error) => notify(error.message),
  };
}

export function networkExportControlsLocked(
  control: NetworkExportControl,
  pending: boolean,
): boolean {
  return control.kind === "active" || pending;
}
