import { useQuery } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { Button } from "../../components/BaseControls";
import { ChannelControls, ChannelDial } from "../../components/ChannelControls";
import { Checkbox } from "../../components/Checkbox";
import {
  mergeChannelSettings,
  radioWindowHz,
  rateMismatch,
  reachesHz,
  squelchLevelDb,
} from "../../components/channelSettings";
import { BTN, BTN_PRIMARY } from "../../components/controls";
import { ANY_FREQUENCY, tuningRange } from "../../components/dial";
import { dialId } from "../../components/FrequencyDial";
import { formatHz, formatMhz, formatSampleRate } from "../../components/format";
import { LevelMeter } from "../../components/LevelMeter";
import { SettingRow } from "../../components/Settings";
import { devicesQuery } from "../../lib/api";
import { useLevelStore } from "../../lib/levels";
import type { DeviceSet, PatchNode, PatchNodeOf } from "../../lib/types";
import { type ChannelEdit, useChannelPatch } from "../../lib/useChannelPatch";
import { forStream, useDevicePatch } from "../../lib/useDevicePatch";
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
import { tuneDelta } from "./deviceNode";
import { FaceBody, FaceFooter, NodeShell } from "./NodeShell";

type ChannelNodeData = PatchNodeOf<"channel">["data"];

export function ChannelFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const set = deviceSetOf(workspace, node.id);
  const levels = useLevelStore((state) => (set === null ? undefined : state.byDeviceSet[set.id]));
  const attached = useQuery(devicesQuery());
  const { applyEdit } = useChannelPatch();
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
  const live = channel === null || set === null ? null : { deviceSet: set.id, id: channel.id };
  const settings =
    channel?.settings ?? workspace.savedChannels.get(node.id) ?? descriptor?.defaults ?? null;
  const onEdit = (edit: ChannelEdit): void => {
    if (live !== null) {
      applyEdit(live.deviceSet, live.id, edit);
      return;
    }
    if (settings !== null) {
      workspace.saveChannel(
        node.id,
        mergeChannelSettings(settings, typeof edit === "function" ? edit(settings) : edit),
      );
    }
  };
  const frequencyHz = settings?.frequency_hz ?? null;
  const wantedRate = rateMismatch(descriptor, set?.settings.sample_rate);
  const window = radioWindowHz(centerHz, set?.settings.sample_rate, descriptor);
  const unreachable =
    set !== null &&
    frequencyHz !== null &&
    (channel?.out_of_band ?? !reachesHz(frequencyHz, window));
  const locked = node.data.tuning_locked ?? false;
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
    wrongRate: wantedRate !== null,
  });
  const action = live === null ? channelBindingAction(binding) : null;

  return (
    <NodeShell node={node} title={name} category="channel" subtitle={status}>
      <FaceBody>
        {wantedRate !== null && set !== null && (
          <RateMismatch name={name} set={set} wanted={wantedRate} />
        )}
        {unreachable && set !== null && frequencyHz !== null && (
          <OutOfBand set={set} stream={source?.stream ?? 0} frequencyHz={frequencyHz} />
        )}
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
              locked={locked}
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
  wrongRate,
}: {
  live: boolean;
  binding: ChannelBinding;
  unreachable: boolean;
  wrongRate: boolean;
}) {
  if (wrongRate) {
    return <span className="text-danger">wrong rate</span>;
  }
  if (!live) {
    return <span title={channelBindingHint(binding)}>{channelBindingStatus(binding)}</span>;
  }
  if (unreachable) {
    return <span className="text-warn">out of band</span>;
  }
  return undefined;
}

function FaceNotice({
  tone,
  role,
  title,
  label,
  action,
}: {
  tone: "warn" | "danger";
  role: "status" | "alert";
  title: string;
  label: string;
  action: ReactNode;
}) {
  return (
    <div
      role={role}
      title={title}
      className={`flex flex-wrap items-center justify-between gap-2 border-b px-2 py-1 ${
        tone === "danger"
          ? "border-danger/40 bg-danger/10 text-danger"
          : "border-warn/40 bg-warn/10 text-warn"
      }`}
    >
      <span className="font-mono text-[10px] tracking-[0.09em] uppercase">{label}</span>
      {action}
    </div>
  );
}

function OutOfBand({
  set,
  stream,
  frequencyHz,
}: {
  set: DeviceSet;
  stream: number;
  frequencyHz: number;
}) {
  const { applyPatch } = useDevicePatch();
  const reachable = tunerReaches(set, frequencyHz);
  return (
    <FaceNotice
      tone="warn"
      role="status"
      title={
        reachable
          ? `${set.device.label} is tuned somewhere it cannot hear ${formatHz(frequencyHz)}; the decoder keeps its own frequency and stays silent until the radio comes back over it`
          : `${set.device.label} cannot reach ${formatHz(frequencyHz)} at all, so another radio has to carry this decoder`
      }
      label="Radio is elsewhere"
      action={
        reachable ? (
          <Button
            type="button"
            className={BTN}
            onClick={() => applyPatch(set.id, tuneDelta(set.capabilities, stream, frequencyHz))}
          >
            Tune to {formatMhz(frequencyHz)}
          </Button>
        ) : (
          <span className="text-xs">needs another radio</span>
        )
      }
    />
  );
}

function tunerReaches(set: DeviceSet, hz: number): boolean {
  const ranges = set.capabilities.freq_ranges;
  return ranges.length === 0 || ranges.some((range) => hz >= range.min && hz <= range.max);
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
      ? `exactly ${formatSampleRate(wanted.min)}`
      : Number.isFinite(wanted.max)
        ? `${formatSampleRate(wanted.min)} – ${formatSampleRate(wanted.max)}`
        : `at least ${formatSampleRate(wanted.min)}`;
  return (
    <FaceNotice
      tone="danger"
      role="alert"
      title={
        offered === null
          ? `${name} reads the radio's own samples, so the radio has to run ${range}; this radio offers no rate in that range, so another one has to carry it`
          : `${name} reads the radio's own samples, so the radio has to run ${range}; at ${formatSampleRate(set.settings.sample_rate ?? 0)} it decodes nothing`
      }
      label={`Rate must be ${range}`}
      action={
        offered === null ? (
          <span className="text-xs">needs another radio</span>
        ) : (
          <Button
            type="button"
            className={BTN}
            onClick={() => applyPatch(set.id, { sample_rate: offered })}
          >
            Set {formatSampleRate(offered)}
          </Button>
        )
      }
    />
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
