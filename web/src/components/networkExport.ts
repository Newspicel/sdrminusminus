import { networkExportDeviceSet } from "../lib/api";
import { pushToast } from "../lib/toasts";
import type { NetworkExportAction, NetworkExportSettings, NetworkExportStatus } from "../lib/types";

export type NetworkExportControl =
  | { kind: "unavailable" }
  | { kind: "ready" }
  | { kind: "active"; status: NetworkExportStatus }
  | { kind: "busy"; owner: string };

interface NetworkExportTarget {
  deviceSet: number;
  stream: number;
}

type NetworkExportRequest = typeof networkExportDeviceSet;

export function deriveNetworkExportControl(
  set: { status: string; network_export?: NetworkExportStatus | null } | null,
  node: string,
): NetworkExportControl {
  if (set === null || set.status !== "running") {
    return { kind: "unavailable" };
  }
  const active = set.network_export;
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
  request: NetworkExportRequest = networkExportDeviceSet,
  notify: (message: string) => void = pushToast,
) {
  return {
    mutationFn: (action: NetworkExportAction) => {
      if (target === null) {
        return Promise.reject(new Error("Wire a running device's IQ into this sink first."));
      }
      return request(target.deviceSet, action, node, target.stream, settings);
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
