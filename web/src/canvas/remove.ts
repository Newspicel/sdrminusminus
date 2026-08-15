import {
  controlTimeMachine,
  deleteChannel,
  deleteDeviceSet,
  networkExportChannel,
  networkExportDeviceSet,
} from "../lib/api";
import { basebandSourceOf, iqSourceOf } from "./binding";
import type { Workspace } from "./context";
import { nodeOf } from "./graph";

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
    } else if (node?.kind === "network_export") {
      const baseband = basebandSourceOf(workspace.graph, id, workspace.devices, workspace.channels);
      if (baseband !== null) {
        if (baseband.channel.network_export?.node === id) {
          await networkExportChannel(
            baseband.deviceSet,
            baseband.channel.id,
            "stop",
            id,
            node.data,
          );
        }
        continue;
      }
      const source = iqSourceOf(workspace.graph, id);
      const set = source === null ? undefined : workspace.devices.get(source.source);
      if (set?.network_export?.node === id) {
        await networkExportDeviceSet(set.id, "stop", id, source?.stream ?? 0, node.data);
      }
    } else if (node?.kind === "time_machine") {
      const source = iqSourceOf(workspace.graph, id);
      const set = source === null ? undefined : workspace.devices.get(source.source);
      if (set?.time_machine?.node === id) {
        await controlTimeMachine(set.id, "disarm", id, source?.stream ?? 0, node.data);
      }
    }
  }
}
