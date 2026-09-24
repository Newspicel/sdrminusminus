import { formatHz } from "../../components/format";
import type { PatchNode } from "../../lib/types";
import { useChannelPatch } from "../../lib/useChannelPatch";
import { useWorkspaceContext } from "../context";
import { decoderReplacements } from "../palette";
import { deviceSetOf } from "../workspaceDevice";
import { ChannelPicker } from "./ChannelPicker";
import { swapDecoder } from "./decoderSwap";

export function ReplaceDecoder({ node, onClose }: { node: PatchNode; onClose: () => void }) {
  const workspace = useWorkspaceContext();
  const { applyEdit } = useChannelPatch();
  if (node.kind !== "channel") {
    return null;
  }
  const typeId = node.data.channel_type;
  const descriptor = workspace.context.channelTypes.find((type) => type.type_id === typeId);
  const channel = workspace.channels.get(node.id) ?? null;
  const set = deviceSetOf(workspace, node.id);
  const settings = channel?.settings ?? workspace.savedChannels.get(node.id) ?? null;

  return (
    <ChannelPicker
      title="Replace the decoder"
      note={`${descriptor?.name ?? typeId.toUpperCase()}${
        settings === null ? "" : `: ${formatHz(settings.frequency_hz)}`
      }`}
      groups={decoderReplacements(workspace.context.channelTypes, typeId)}
      onChannel={(channelType) => {
        const wanted = workspace.context.channelTypes.find((type) => type.type_id === channelType);
        if (wanted !== undefined) {
          swapDecoder({
            context: workspace.context,
            node,
            descriptor: wanted,
            live: channel === null || set === null ? null : { deviceSet: set.id, channel },
            saved: settings,
            applyEdit,
            saveChannel: workspace.saveChannel,
            edit: workspace.edit,
          });
        }
        onClose();
      }}
      onClose={onClose}
    />
  );
}
