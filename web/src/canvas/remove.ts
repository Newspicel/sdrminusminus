// Taking a node off the patch has to take what it was driving with it. Applying a patch is
// deliberately additive — it never closes a radio or deletes a channel (CANVAS §4) — so removal
// is the only gesture that does, and without it a removed channel would keep running in the
// engine forever and a radio could never be closed at all.
import { deleteChannel, deleteDeviceSet } from "../lib/api";
import { iqSourceOf } from "./binding";
import type { Workspace } from "./context";
import { nodeOf } from "./graph";

/**
 * Close the engine objects these nodes were driving.
 *
 * It runs *before* the nodes leave the patch, and a failure is left to propagate: the node then
 * stays, which is the honest outcome — the patch must not draw the radio as gone while it is
 * still streaming. Every path that removes a node goes through here (the face's ✕, Backspace,
 * the context menu), so the three cannot drift into meaning different things.
 */
/**
 * Let go of the radio a device node names: close it in the engine, then unbind the node.
 *
 * Unbinding alone would leave the radio open with nothing on the canvas pointing at it — still
 * claiming the USB device (exclusive, so nothing else can open it), still streaming, still
 * costing a DSP thread, and reachable only by binding a node back to it. Nothing about the
 * patch is lost by closing: the node and its wires stay, and this node's device settings are
 * saved per node, so naming a radio again re-opens it and `restore_device` puts them back.
 *
 * The close comes first and its failure propagates, exactly as node removal does: the unbind
 * must not happen if the radio is still running, or the patch would draw a radio nobody owns.
 */
export async function releaseRadio(
  workspace: Workspace,
  node: string,
  unbind: () => void,
): Promise<void> {
  await closeEngineObjects(workspace, [node]);
  unbind();
}

export async function closeEngineObjects(
  workspace: Workspace,
  ids: readonly string[],
): Promise<void> {
  for (const id of ids) {
    const node = nodeOf(workspace.graph, id);
    if (node?.kind === "device") {
      const set = workspace.devices.get(id);
      if (set !== undefined) {
        await deleteDeviceSet(set.id);
      }
    } else if (node?.kind === "channel") {
      const channel = workspace.channels.get(id);
      const owner = iqSourceOf(workspace.graph, id)?.source;
      const set = owner === undefined ? undefined : workspace.devices.get(owner);
      if (channel !== undefined && set !== undefined) {
        await deleteChannel(set.id, channel.id);
      }
    }
  }
}
