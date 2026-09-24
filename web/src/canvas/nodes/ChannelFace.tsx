import { useQuery } from "@tanstack/react-query";
import { Button } from "../../components/BaseControls";
import { ChannelControls, ChannelDial } from "../../components/ChannelControls";
import { Checkbox } from "../../components/Checkbox";
import {
  channelHasAudio,
  channelWidthHz,
  radioWindowHz,
  reachesHz,
  squelchLevelDb,
} from "../../components/channelSettings";
import { BTN_PRIMARY } from "../../components/controls";
import { ANY_FREQUENCY, tuningRange } from "../../components/dial";
import { dialId } from "../../components/FrequencyDial";
import { formatHz } from "../../components/format";
import { LevelMeter } from "../../components/LevelMeter";
import { SettingRow } from "../../components/Settings";
import { devicesQuery } from "../../lib/api";
import { useDecodedKind } from "../../lib/decoded";
import { useLevelStore } from "../../lib/levels";
import { trackedBy, useSatelliteStore } from "../../lib/satellite";
import type { PatchNode, PatchNodeOf } from "../../lib/types";
import { channelSettingsOf, liveChannelOf, useChannelEdit } from "../../lib/useChannelEdit";
import type { ChannelEdit } from "../../lib/useChannelPatch";
import { iqLanesOf, tuningControllerOf } from "../binding";
import { useWorkspaceContext } from "../context";
import { nodeOf, patchNode } from "../graph";
import { deviceSetOf, laneOf } from "../workspaceDevice";
import { keepsCalls } from "./callRecording";
import {
  type ChannelBinding,
  channelBinding,
  channelBindingAction,
  channelBindingHint,
  channelBindingStatus,
  radioIsAttached,
  radioRefsOf,
} from "./channelNode";
import { laneCenterHz, laneRateHz } from "./deviceNode";
import { FaceBody, FaceFooter, NodeShell } from "./NodeShell";

const AUDIO_FACE_W = 460;

type ChannelNodeData = PatchNodeOf<"channel">["data"];

export function ChannelFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const set = deviceSetOf(workspace, node.id);
  const levels = useLevelStore((state) => (set === null ? undefined : state.byDeviceSet[set.id]));
  const attached = useQuery(devicesQuery());
  const editChannel = useChannelEdit();
  const broadcasts = useDecodedKind("broadcast");
  const tracked = useSatelliteStore((store) => trackedBy(store.byNode, node.id));
  if (node.kind !== "channel") {
    return null;
  }

  const typeId = node.data.channel_type;
  const descriptor = workspace.context.channelTypes.find((type) => type.type_id === typeId);
  const name = descriptor?.name ?? typeId.toUpperCase();
  const channel = workspace.channels.get(node.id) ?? null;
  const lanes = iqLanesOf(workspace.graph, node.id);
  const source = laneOf(workspace, node.id);
  const references = radioRefsOf(workspace.graph, node.id);
  const binding = channelBinding({
    wired: lanes.length > 0,
    open: set !== null,
    named: references.length > 0,
    attached: radioIsAttached(references, attached.data?.devices ?? []),
  });
  const centerHz = set === null ? null : laneCenterHz(set, source?.stream ?? 0);
  const live = liveChannelOf(workspace, node.id);
  const settings = channelSettingsOf(workspace, node.id);
  const onEdit = (edit: ChannelEdit): void => editChannel(node.id, edit);
  const frequencyHz = settings?.frequency_hz ?? null;
  const spanHz = set === null ? undefined : laneRateHz(set, source?.stream ?? 0);
  const window = radioWindowHz(centerHz, spanHz, descriptor);
  const unreachable =
    set !== null &&
    frequencyHz !== null &&
    (channel?.out_of_band ?? !reachesHz(frequencyHz, window));
  const locked = node.data.tuning_locked ?? false;
  const scanned =
    channel !== null &&
    (set?.scanners?.some(
      (scanner) => scanner.error == null && scanner.settings.channel === channel.id,
    ) ??
      false);
  const controller = tuningControllerOf(workspace.graph, node.id);
  const editNode = (next: Partial<ChannelNodeData>): void =>
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, node.id, (current) =>
        current.kind === "channel" ? { ...current, data: { ...current.data, ...next } } : current,
      ),
    }));

  const carrier =
    lanes.length > 1 && source !== null && set !== null
      ? (nodeOf(workspace.graph, source.source)?.label ?? set.device.label)
      : null;
  const status = faceStatus({
    live: live !== null,
    binding,
    unreachable,
    driver: scanned ? "scanning" : (tracked?.name ?? (tracked === null ? null : "satellite")),
    carrier,
  });
  const widthHz = channelWidthHz(settings?.params, descriptor);
  const action = live === null ? channelBindingAction(binding) : null;

  return (
    <NodeShell
      node={node}
      title={name}
      category="channel"
      subtitle={status}
      badge={
        widthHz === null ? undefined : <span title="Channel bandwidth">{formatHz(widthHz)}</span>
      }
      width={channelHasAudio(descriptor) ? AUDIO_FACE_W : undefined}
    >
      <FaceBody>
        {settings !== null && (
          <div className="@container flex flex-col gap-1.5 border-b border-line p-2">
            <ChannelDial
              hz={settings.frequency_hz}
              descriptor={descriptor}
              spanHz={spanHz ?? null}
              centerHz={centerHz}
              range={set === null ? ANY_FREQUENCY : tuningRange(set.capabilities)}
              dialId={dialId(node.id)}
              wheelTunes={workspace.selected === node.id}
              locked={locked}
              heldBy={controller === null ? null : (tracked?.name ?? controller)}
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
  driver,
  carrier,
}: {
  live: boolean;
  binding: ChannelBinding;
  unreachable: boolean;
  driver: string | null;
  carrier: string | null;
}) {
  if (!live) {
    return binding === "unwired" ? undefined : (
      <span title={channelBindingHint(binding)}>{channelBindingStatus(binding)}</span>
    );
  }
  if (driver !== null) {
    return <span title="Tuned by the node on its control input">{driver}</span>;
  }
  if (unreachable) {
    return <span className="text-warn">out of band</span>;
  }
  if (carrier !== null) {
    return <span title="The radio carrying this decoder now">{carrier}</span>;
  }
  return undefined;
}
