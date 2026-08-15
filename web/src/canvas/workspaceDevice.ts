// The radio a node is about, resolved through the workspace. Kept out of `context.tsx` so that
// file exports only the provider and its hook — a non-component export there costs Fast Refresh
// the component state it would otherwise preserve.
import type { DeviceSet } from "../lib/types";
import { deviceNodeOf } from "./binding";
import type { Workspace } from "./context";

/** The device set behind a node, following the wire when the node is a channel, a sink or a
 * scanner (`deviceNodeOf`). Returns `null` while the radio is absent — the face renders
 * disconnected rather than empty. */
export function deviceSetOf(workspace: Workspace, node: string): DeviceSet | null {
  const owner = deviceNodeOf(workspace.graph, node);
  return owner === null ? null : (workspace.devices.get(owner) ?? null);
}
