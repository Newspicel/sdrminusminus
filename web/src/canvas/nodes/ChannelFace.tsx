import { useQuery } from "@tanstack/react-query";
import { Button } from "../../components/BaseControls";
import { ChannelControls } from "../../components/ChannelControls";
import { Checkbox } from "../../components/Checkbox";
import { channelHasVideo, rateMismatch } from "../../components/channelSettings";
import { BTN, BTN_PRIMARY } from "../../components/controls";
import { formatMhz, formatSignedKhz } from "../../components/format";
import { LevelMeter } from "../../components/LevelMeter";
import { SettingRow, Settings } from "../../components/Settings";
import { devicesQuery } from "../../lib/api";
import { useLevelStore } from "../../lib/levels";
import type { ChannelDescriptor, DeviceSet, PatchGraph, PatchNode } from "../../lib/types";
import { forStream, useDevicePatch } from "../../lib/useDevicePatch";
import { iqSourceOf, targetsOf } from "../binding";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { deviceSetOf } from "../workspaceDevice";
import { keepsCalls } from "./callRecording";
import {
  type ChannelBinding,
  channelBinding,
  channelBindingAction,
  channelBindingLabel,
  channelBindingSaid,
  radioIsAttached,
  radioRefOf,
} from "./channelNode";
import { FaceBody, NodeShell } from "./NodeShell";

export function ChannelFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const set = deviceSetOf(workspace, node.id);
  const levels = useLevelStore((state) => (set === null ? undefined : state.byDeviceSet[set.id]));
  const attached = useQuery(devicesQuery());
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
  const offsetHz = channel?.settings.offset_hz ?? 0;
  const readout = centerHz === null ? formatSignedKhz(offsetHz) : formatMhz(centerHz + offsetHz);
  const wantedRate = rateMismatch(descriptor, set?.settings.sample_rate);
  const unwired = unwiredOutputs(workspace.graph, node.id, descriptor);
  const editRecording = (on: boolean) => {
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, node.id, (current) =>
        current.kind === "channel"
          ? { ...current, data: { ...current.data, record_calls: on } }
          : current,
      ),
    }));
  };

  return (
    <NodeShell
      node={node}
      title={name}
      category="channel"
      subtitle={
        channel === null ? (
          channelBindingLabel(binding)
        ) : (
          <span className="font-mono tabular-nums">{readout}</span>
        )
      }
      live={channel !== null}
    >
      <FaceBody>
        {wantedRate !== null && set !== null && (
          <RateMismatch name={name} set={set} wanted={wantedRate} />
        )}
        {channel === null || set === null ? (
          <Unbound binding={binding} onApply={workspace.apply} />
        ) : (
          <>
            <div className="px-2 pt-2">
              <LevelMeter level={levels?.[channel.id]} squelchDb={channel.settings.squelch_db} />
            </div>
            <ChannelControls
              deviceSet={set.id}
              channel={channel}
              descriptor={descriptor}
              spanHz={set.settings.sample_rate ?? null}
            />
            {unwired.map((reason) => (
              <p key={reason} className="legend px-2 pb-2">
                {reason}
              </p>
            ))}
            {keepsCalls(descriptor) && (
              <Settings className="border-t border-line p-2">
                <SettingRow label="Record calls">
                  <Checkbox
                    label="Record calls"
                    checked={node.data.record_calls ?? false}
                    onChange={editRecording}
                  />
                </SettingRow>
              </Settings>
            )}
          </>
        )}
      </FaceBody>
    </NodeShell>
  );
}

function unwiredOutputs(
  graph: PatchGraph,
  node: string,
  descriptor: ChannelDescriptor | undefined,
): string[] {
  const reaches = (port: string): boolean => targetsOf(graph, node, port).length > 0;
  return channelHasVideo(descriptor) && !reaches("video") ? ["video out reaches no screen"] : [];
}

function RateMismatch({
  name,
  set,
  wanted,
}: {
  name: string;
  set: DeviceSet;
  wanted: { min: number; max: number };
}) {
  const { applyPatch } = useDevicePatch();
  const offered = nearestRate(set, wanted);
  const range =
    wanted.min === wanted.max
      ? `exactly ${mhz(wanted.min)} MHz`
      : `between ${mhz(wanted.min)} and ${mhz(wanted.max)} MHz`;
  return (
    <div
      role="alert"
      className="flex flex-col items-start gap-1.5 border-b border-danger/40 bg-danger/10 px-2 py-1.5 text-xs text-danger"
    >
      <p>
        {name} reads the radio's own samples, so the radio has to run {range}. At{" "}
        <span className="font-mono tabular-nums">{mhz(set.settings.sample_rate ?? 0)}</span> MHz it
        decodes nothing at all.
      </p>
      {offered === null ? (
        <p>
          This radio offers no rate in that range, so it cannot carry {name}. Another radio has to.
        </p>
      ) : (
        <Button
          type="button"
          className={BTN}
          onClick={() => applyPatch(set.id, { sample_rate: offered })}
        >
          Set {set.device.label} to {mhz(offered)} MHz
        </Button>
      )}
    </div>
  );
}

function nearestRate(set: DeviceSet, wanted: { min: number; max: number }): number | null {
  const rates = set.capabilities.sample_rates;
  if (rates.length === 0) {
    return wanted.min;
  }
  const inside = rates.filter((rate) => rate >= wanted.min && rate <= wanted.max);
  return inside.length === 0 ? null : Math.min(...inside);
}

function mhz(hz: number): string {
  return (hz / 1e6).toFixed(3);
}

function Unbound({ binding, onApply }: { binding: ChannelBinding; onApply: () => void }) {
  const action = channelBindingAction(binding);
  return (
    <div className="flex flex-col items-start gap-2 p-3">
      <p className="text-sm text-ink-dim">{channelBindingSaid(binding)}</p>
      {action !== null && (
        <Button type="button" className={BTN_PRIMARY} onClick={onApply}>
          {action}
        </Button>
      )}
    </div>
  );
}
