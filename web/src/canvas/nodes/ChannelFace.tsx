import { useQuery } from "@tanstack/react-query";
import { Button } from "../../components/BaseControls";
import { ChannelControls, ChannelDial } from "../../components/ChannelControls";
import { Checkbox } from "../../components/Checkbox";
import { radioWindowHz, reachesHz, squelchLevelDb } from "../../components/channelSettings";
import { BTN_PRIMARY } from "../../components/controls";
import { ANY_FREQUENCY, tuningRange } from "../../components/dial";
import { dialId } from "../../components/FrequencyDial";
import { LevelMeter } from "../../components/LevelMeter";
import { SettingRow } from "../../components/Settings";
import { devicesQuery } from "../../lib/api";
import { useDecodedKind } from "../../lib/decoded";
import { useLevelStore } from "../../lib/levels";
import type { PatchNode, PatchNodeOf } from "../../lib/types";
import { channelSettingsOf, liveChannelOf, useChannelEdit } from "../../lib/useChannelEdit";
import type { ChannelEdit } from "../../lib/useChannelPatch";
import { forStream } from "../../lib/useDevicePatch";
import { iqSourceOf } from "../binding";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { deviceSetOf } from "../workspaceDevice";
import { keepsCalls } from "./callRecording";
import {
  type ChannelBinding,
  channelBinding,
  channelBindingAction,
  channelBindingHint,
  channelBindingStatus,
  radioIsAttached,
  radioRefOf,
} from "./channelNode";
import { FaceBody, FaceFooter, NodeShell } from "./NodeShell";

type ChannelNodeData = PatchNodeOf<"channel">["data"];

export function ChannelFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const set = deviceSetOf(workspace, node.id);
  const levels = useLevelStore((state) => (set === null ? undefined : state.byDeviceSet[set.id]));
  const attached = useQuery(devicesQuery());
  const editChannel = useChannelEdit();
  const broadcasts = useDecodedKind("broadcast");
  if (node.kind !== "channel") {
    return null;
  }

  const typeId = node.data.channel_type;
  const descriptor = workspace.context.channelTypes.find((type) => type.type_id === typeId);
  const name = descriptor?.name ?? typeId.toUpperCase();
  const channel = workspace.channels.get(node.id) ?? null;
  const source = iqSourceOf(workspace.graph, node.id);
  const wired = source !== null;
  const reference = radioRefOf(workspace.graph, node.id);
  const binding = channelBinding({
    wired,
    open: set !== null,
    named: reference !== null,
    attached: radioIsAttached(reference, attached.data?.devices ?? []),
  });
  const centerHz =
    set === null
      ? null
      : (forStream(set.settings, source?.stream ?? 0, set.capabilities.per_stream).center_hz ??
        null);
  const live = liveChannelOf(workspace, node.id);
  const settings = channelSettingsOf(workspace, node.id);
  const onEdit = (edit: ChannelEdit): void => editChannel(node.id, edit);
  const frequencyHz = settings?.frequency_hz ?? null;
  const window = radioWindowHz(centerHz, set?.settings.sample_rate, descriptor);
  const unreachable =
    set !== null &&
    frequencyHz !== null &&
    (channel?.out_of_band ?? !reachesHz(frequencyHz, window));
  const locked = node.data.tuning_locked ?? false;
  const driven =
    channel !== null &&
    set?.scanner != null &&
    set.scanner.error == null &&
    set.scanner.settings.channel === channel.id;
  const editNode = (next: Partial<ChannelNodeData>): void =>
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, node.id, (current) =>
        current.kind === "channel" ? { ...current, data: { ...current.data, ...next } } : current,
      ),
    }));

  const status = faceStatus({
    live: live !== null,
    binding,
    unreachable,
    driven,
  });
  const action = live === null ? channelBindingAction(binding) : null;

  return (
    <NodeShell node={node} title={name} category="channel" subtitle={status}>
      <FaceBody>
        {settings !== null && (
          <div className="@container flex flex-col gap-1.5 border-b border-line p-2">
            <ChannelDial
              hz={settings.frequency_hz}
              descriptor={descriptor}
              spanHz={set?.settings.sample_rate ?? null}
              centerHz={centerHz}
              range={set === null ? ANY_FREQUENCY : tuningRange(set.capabilities)}
              dialId={dialId(node.id)}
              wheelTunes={workspace.selected === node.id}
              locked={locked || driven}
              onTune={(frequency_hz) => onEdit({ frequency_hz })}
              onLock={(tuning_locked) => editNode({ tuning_locked })}
            />
            {live !== null && (
              <LevelMeter level={levels?.[live.id]} squelchDb={squelchLevelDb(settings.squelch)} />
            )}
          </div>
        )}
        {settings !== null && (
          <ChannelControls
            settings={settings}
            descriptor={descriptor}
            broadcast={
              broadcasts.find(
                (record) => record.device_set === live?.deviceSet && record.channel === live?.id,
              )?.event.data
            }
            onEdit={onEdit}
            extra={
              keepsCalls(descriptor) && (
                <SettingRow
                  label="Record calls"
                  title="Save each call the decoder hears as its own audio file"
                >
                  <Checkbox
                    label="Record calls"
                    checked={node.data.record_calls ?? false}
                    onChange={(record_calls) => editNode({ record_calls })}
                  />
                </SettingRow>
              )
            }
          />
        )}
      </FaceBody>
      {action !== null && (
        <FaceFooter>
          <Button
            type="button"
            className={BTN_PRIMARY}
            title={channelBindingHint(binding)}
            onClick={workspace.apply}
          >
            {action}
          </Button>
        </FaceFooter>
      )}
    </NodeShell>
  );
}

function faceStatus({
  live,
  binding,
  unreachable,
  driven,
}: {
  live: boolean;
  binding: ChannelBinding;
  unreachable: boolean;
  driven: boolean;
}) {
  if (!live) {
    return <span title={channelBindingHint(binding)}>{channelBindingStatus(binding)}</span>;
  }
  if (driven) {
    return <span title="A scanner is tuning this decoder">scanning</span>;
  }
  if (unreachable) {
    return <span className="text-warn">out of band</span>;
  }
  return undefined;
}
