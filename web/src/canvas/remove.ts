// Taking a node off the patch has to take what it was driving with it. Applying a patch is
// deliberately additive — it never closes a radio or deletes a channel (CANVAS §4) — so removal
// is the only gesture that does, and without it a removed channel would keep running in the
// engine forever and a receiver could never be closed at all.
import { deleteChannel, deleteDeviceSet } from "../lib/api";
import { sourcesOf } from "./binding";
import type { Station } from "./context";
import { nodeOf } from "./graph";

/**
 * Close the engine objects these nodes were driving.
 *
 * It runs *before* the nodes leave the patch, and a failure is left to propagate: the node then
 * stays, which is the honest outcome — the patch must not draw the radio as gone while it is
 * still streaming. Every path that removes a node goes through here (the face's ✕, Backspace,
 * the context menu), so the three cannot drift into meaning different things.
 */
export async function closeEngineObjects(station: Station, ids: readonly string[]): Promise<void> {
  for (const id of ids) {
    const node = nodeOf(station.graph, id);
    if (node?.kind === "device") {
      const set = station.devices.get(id);
      if (set !== undefined) {
        await deleteDeviceSet(set.id);
      }
    } else if (node?.kind === "channel") {
      const channel = station.channels.get(id);
      const owner = sourcesOf(station.graph, id, "iq")[0];
      const set = owner === undefined ? undefined : station.devices.get(owner);
      if (channel !== undefined && set !== undefined) {
        await deleteChannel(set.id, channel.id);
      }
    }
  }
}
