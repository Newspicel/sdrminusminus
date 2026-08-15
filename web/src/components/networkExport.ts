import type { NetworkExportStatus } from "../lib/types";

export type NetworkExportControl =
  | { kind: "unavailable" }
  | { kind: "ready" }
  | { kind: "active"; status: NetworkExportStatus }
  | { kind: "busy"; owner: string };

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
