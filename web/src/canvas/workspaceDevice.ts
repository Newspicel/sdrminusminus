import type { DeviceSet } from "../lib/types";
import { deviceNodeOf } from "./binding";
import type { Workspace } from "./context";

export function deviceSetOf(workspace: Workspace, node: string): DeviceSet | null {
  const owner = deviceNodeOf(workspace.graph, node);
  return owner === null ? null : (workspace.devices.get(owner) ?? null);
}
