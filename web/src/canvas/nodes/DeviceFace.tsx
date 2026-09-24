import { Collapsible } from "@base-ui/react/collapsible";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link2, Lock } from "lucide-react";
import { Button } from "../../components/BaseControls";
import { BTN_PRIMARY, BTN_QUIET, type Options } from "../../components/controls";
import { DevOnly } from "../../components/DevOnly";
import { deviceId } from "../../components/devices";
import { inTuningRange, isTunable, tuningRange } from "../../components/dial";
import { dialId, FrequencyDial } from "../../components/FrequencyDial";
import { formatMhz } from "../../components/format";
import { Icon } from "../../components/Icon";
import { DeviceChoices } from "../../components/OpenRadio";
import { LaneControls, RadioSettings } from "../../components/RadioSettings";
import { Readout, ReadoutRow } from "../../components/Readout";
import { Segmented } from "../../components/Segmented";
import { SettingRow } from "../../components/Settings";
import { TuneTo } from "../../components/TuneTo";
import { TuningLock } from "../../components/TuningLock";
import { createDeviceSet, devicesQuery, STATE_KEY, stateQuery } from "../../lib/api";
import { queueSummary, usePipelineHealth } from "../../lib/pipeline";
import { toastError } from "../../lib/toasts";
import type {
  DeviceInfo,
  DeviceRef,
  DeviceSet,
  PatchNode,
  PatchNodeOf,
  Tuning,
} from "../../lib/types";
import { useDevicePatch } from "../../lib/useDevicePatch";
import { useRadioTune } from "../../lib/useRadioTune";
import { claimedDevices, deviceRefOf, refMatches } from "../binding";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { releaseRadio } from "../remove";
import { arrayHolding } from "./arrayNode";
import {
  autoTuning,
  bondSaid,
  clippingSaid,
  coherentLanes,
  faultSaid,
  type Hearing,
  hasLaneControls,
  hearing,
  lanesMerged,
  lockStream,
  refLabel,
  refusalSaid,
  type TunerDial,
  tunerDials,
  tuningDelta,
} from "./deviceNode";
import { FaceBody, FaceFooter, NodeShell, useFaceActive } from "./NodeShell";

type DeviceNodeData = PatchNodeOf<"device">["data"];

const TUNING_MODES: Options<Tuning> = [
  { value: "auto", label: "Auto", title: "Tune a decoder, the radio follows" },
  { value: "manual", label: "Manual", title: "The radio stays where you set it" },
];

function TuningMode({ set, stream }: { set: DeviceSet; stream: number }) {
  const { applyPatch } = useDevicePatch();
  return (
    <SettingRow label="Tuning" title="Auto: tune your decoders and the radio follows them">
      <Segmented
        label="Tuning"
        value={autoTuning(set, stream) ? "auto" : "manual"}
        options={TUNING_MODES}
        onChange={(tuning) => applyPatch(set.id, tuningDelta(set.capabilities, stream, tuning))}
      />
    </SettingRow>
  );
}

interface TunerProps {
  node: string;
  set: DeviceSet;
  lockedStreams: readonly number[];
  onLock: (stream: number, locked: boolean) => void;
  arrayTuning: boolean;
  advised: ReadonlySet<number>;
}

function DialRow({
  node,
  set,
  dial,
  locked,
  onLock,
}: Omit<TunerProps, "lockedStreams" | "onLock" | "arrayTuning" | "advised"> & {
  dial: TunerDial;
  locked: boolean;
  onLock: (locked: boolean) => void;
}) {
  const active = useFaceActive();
  const radio = useRadioTune({ node, set, stream: dial.stream, tunes: dial.stream });
  const range = tuningRange(set.capabilities);
  const pinned = !isTunable(range);
  const held = pinned || locked;
  const hz = radio.hz ?? dial.hz;
  return (
    <div className="flex min-w-0 items-center gap-2">
      <FrequencyDial
        id={dialId(node, dial.stream)}
        hz={hz}
        range={range}
        disabled={held}
        wheelTunes={active}
        onTune={radio.tune}
      />
      <span className="ml-auto flex shrink-0 items-center gap-1">
        {!pinned && (
          <TuneTo
            title={dial.port === null ? "Type a frequency" : `Type a frequency for ${dial.port}`}
            hz={hz}
            hint={`Reaches ${formatMhz(range.min)} to ${formatMhz(range.max)}`}
            resolve={(entered) => inTuningRange(entered, range)}
            disabled={held}
            onTune={radio.tune}
          />
        )}
        {!pinned && (
          <TuningLock locked={locked} held="Tuning locked" free="Lock tuning" onLock={onLock} />
        )}
      </span>
    </div>
  );
}

function Tuner(props: TunerProps) {
  const { set, lockedStreams, onLock, arrayTuning, advised } = props;
  const merged = lanesMerged(set);
  const bond = merged ? bondSaid(set.capabilities.coherence) : null;
  const controls = merged && hasLaneControls(set.capabilities);
  const dials = merged ? tunerDials(set) : tunerDials(set).slice(0, 1);
  return (
    <>
      {dials.map((dial, index) => {
        const locked = lockedStreams.includes(dial.stream);
        return (
          <div
            key={dial.stream}
            className={`relative col-span-2 grid grid-cols-subgrid gap-y-2.5 ${
              locked
                ? "before:absolute before:inset-y-0 before:-left-2 before:w-0.5 before:bg-accent"
                : ""
            }`}
            title={arrayTuning && !merged ? ARRAY_TUNED : undefined}
          >
            {merged && (
              <LaneRule
                port={dial.port}
                locked={locked}
                bond={index === 0 ? bond : null}
                arrayTuning={index === 0 && arrayTuning}
              />
            )}
            <div className="@container col-span-2 min-w-0">
              <DialRow
                {...props}
                locked={locked}
                dial={dial}
                onLock={(next) => onLock(dial.stream, next)}
              />
            </div>
            {isTunable(tuningRange(set.capabilities)) && (
              <TuningMode set={set} stream={dial.stream} />
            )}
            {controls && (
              <LaneControls active={set} stream={dial.stream} advised={advised.has(dial.stream)} />
            )}
          </div>
        );
      })}
      <div className="col-span-2 -mx-2 border-t border-line" />
    </>
  );
}

function LaneRule({
  port,
  locked,
  bond,
  arrayTuning,
}: {
  port: string | null;
  locked: boolean;
  bond: string | null;
  arrayTuning: boolean;
}) {
  return (
    <div className="legend col-span-2 flex items-center gap-2 leading-none">
      <span className="h-px w-3 bg-line" />
      <span className="flex items-center gap-1 font-mono text-port-iq">
        {port}
        {locked && (
          <span className="text-accent" title="Tuning locked">
            <Icon glyph={Lock} size={12} />
          </span>
        )}
      </span>
      <span className="h-px flex-1 bg-line" />
      {bond !== null && (
        <span
          className="flex items-center gap-1 text-port-iq"
          title="The lanes sample on one clock, so their streams line up in time"
        >
          <Icon glyph={Link2} size={12} />
          {bond}
        </span>
      )}
      {arrayTuning && (
        <span className="flex items-center gap-1 text-port-iq" title={ARRAY_TUNED}>
          <Icon glyph={Link2} size={12} />
          Array
        </span>
      )}
    </div>
  );
}

const ARRAY_TUNED = "Tuning moves the whole Array";

const TONE: Record<Hearing["tone"], string> = {
  ok: "text-ok",
  warn: "text-warn",
  danger: "text-danger",
};

const INSIDE = "Decoders wired to this radio that sit inside its window";

function Heard({ set }: { set: DeviceSet }) {
  const heard = hearing(set);
  return (
    <span
      role="status"
      className={TONE[heard.tone]}
      title={
        heard.tone === "ok"
          ? INSIDE
          : `${INSIDE}. Raise the sample rate, or move the ones it misses to another radio.`
      }
    >
      {heard.heard}/{heard.total}
    </span>
  );
}

function Fault({ set }: { set: DeviceSet }) {
  const said = faultSaid(set);
  return (
    <div role="alert" className="border-t border-line p-2 text-xs text-danger">
      {said == null ? (
        <p className="font-mono">Device fault · {set.error}</p>
      ) : (
        <Collapsible.Root>
          <Collapsible.Trigger className="cursor-pointer text-left">{said}</Collapsible.Trigger>
          <Collapsible.Panel>
            <p className="mt-1 font-mono text-ink-dim">{set.error}</p>
          </Collapsible.Panel>
        </Collapsible.Root>
      )}
    </div>
  );
}

function Refused({ set }: { set: DeviceSet }) {
  const said = refusalSaid(set);
  if (said == null) {
    return null;
  }
  return (
    <div role="alert" className="border-t border-line p-2 text-xs text-danger">
      <Collapsible.Root>
        <Collapsible.Trigger className="cursor-pointer text-left">{said}</Collapsible.Trigger>
        <Collapsible.Panel>
          <p className="mt-1 font-mono text-ink-dim">{set.refused?.error}</p>
        </Collapsible.Panel>
      </Collapsible.Root>
    </div>
  );
}

export function DeviceFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const queryClient = useQueryClient();
  const attached = useQuery(devicesQuery());
  const reference = node.kind === "device" ? (node.data.device ?? null) : null;
  const lockedStreams = node.kind === "device" ? (node.data.locked_streams ?? []) : [];
  const set = workspace.devices.get(node.id) ?? null;
  const onBus =
    reference !== null &&
    (attached.data?.devices ?? []).some((device) => refMatches(reference, device));

  const open = useMutation({
    mutationFn: createDeviceSet,
    onSuccess: () => workspace.apply(),
    onError: (error: Error) => toastError(error),
    onSettled: () => void queryClient.invalidateQueries({ queryKey: STATE_KEY }),
  });

  const editNode = (next: Partial<DeviceNodeData>): void =>
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, node.id, (stored) =>
        stored.kind === "device" ? { ...stored, data: { ...stored.data, ...next } } : stored,
      ),
    }));

  const nameRadio = (chosen: DeviceRef | null): void => editNode({ device: chosen });

  const forget = useMutation({
    mutationFn: () => releaseRadio(workspace, node.id, () => nameRadio(null)),
    onError: (error: Error) => toastError(error),
    onSettled: () => void queryClient.invalidateQueries({ queryKey: STATE_KEY }),
  });

  const bind = (device: DeviceInfo): void => {
    const chosen = deviceRefOf(device);
    nameRadio(chosen);
    if (workspace.deviceSets.some((candidate) => refMatches(chosen, candidate.device))) {
      workspace.apply();
    } else {
      open.mutate(deviceId(device));
    }
  };

  const openNetwork = useMutation({
    mutationFn: async (id: string): Promise<DeviceInfo | null> => {
      const created = await createDeviceSet(id);
      const state = await queryClient.fetchQuery(stateQuery());
      return state.device_sets.find((candidate) => candidate.id === created)?.device ?? null;
    },
    onSuccess: (device) => {
      if (device !== null) {
        nameRadio(deviceRefOf(device));
      }
      workspace.apply();
    },
    onError: (error: Error) => toastError(error),
    onSettled: () => void queryClient.invalidateQueries({ queryKey: STATE_KEY }),
  });

  if (reference === null) {
    return (
      <NodeShell node={node} title="Device" category="source">
        <FaceBody>
          <div className="flex flex-col gap-2 p-2">
            <DeviceChoices
              onChoose={bind}
              onAddNetwork={(id) => openNetwork.mutate(id)}
              busy={open.isPending || openNetwork.isPending}
              error={open.error?.message ?? openNetwork.error?.message ?? null}
              claimed={claimedDevices(workspace.graph, node.id)}
            />
          </div>
        </FaceBody>
      </NodeShell>
    );
  }

  if (set === null) {
    return (
      <NodeShell
        node={node}
        title="Device"
        category="source"
        subtitle={onBus ? "not open" : "disconnected"}
      >
        <FaceBody>
          <p className="p-3 font-mono text-sm text-ink">{refLabel(reference)}</p>
        </FaceBody>
        <FaceFooter>
          <Button
            type="button"
            className={BTN_QUIET}
            title="Free this node so you can pick a different radio"
            onClick={() => forget.mutate()}
            disabled={forget.isPending}
          >
            Forget radio
          </Button>
          <Button
            type="button"
            className={BTN_PRIMARY}
            title={
              onBus
                ? "Open this radio and start the channels wired to it"
                : "Nothing to open until the radio is plugged back in"
            }
            onClick={() => workspace.apply()}
            disabled={!onBus}
          >
            Open radio
          </Button>
        </FaceFooter>
      </NodeShell>
    );
  }

  const array = arrayHolding(workspace.graph, node.id);
  const arrayTuning = array !== null && workspace.devices.has(array);
  const advised = coherentLanes(workspace.graph, node.id);

  return (
    <NodeShell
      node={node}
      title={set.device.label}
      category="source"
      subtitle={<Heard set={set} />}
    >
      <FaceBody>
        <RadioSettings
          active={set}
          className="p-2"
          lanesShown={lanesMerged(set)}
          advised={advised}
          lead={
            <Tuner
              node={node.id}
              set={set}
              arrayTuning={arrayTuning}
              advised={advised}
              lockedStreams={lockedStreams}
              onLock={(stream, next) =>
                editNode({ locked_streams: lockStream(lockedStreams, stream, next) })
              }
            />
          }
        />

        <DevOnly>
          <DeviceHealth set={set} />
        </DevOnly>

        {set.error != null && <Fault set={set} />}
        <Refused set={set} />
      </FaceBody>
      <FaceFooter>
        <Button
          type="button"
          className={BTN_QUIET}
          title="Close this radio and free the node: the device is released and the wires stay drawn"
          onClick={() => forget.mutate()}
          disabled={forget.isPending}
        >
          {forget.isPending ? "Closing…" : "Forget radio"}
        </Button>
      </FaceFooter>
    </NodeShell>
  );
}

function DeviceHealth({ set }: { set: DeviceSet }) {
  const health = usePipelineHealth((state) => state.health);
  const summary = queueSummary(health, set.id);
  const overruns = set.overruns ?? 0;
  const clipping = clippingSaid(set);
  if (summary === null && overruns === 0 && clipping === null) {
    return null;
  }
  return (
    <Readout>
      {clipping !== null && (
        <ReadoutRow label="Clipping" title="The ADC is at full scale. Lower the gain.">
          {clipping}
        </ReadoutRow>
      )}
      {summary !== null && (
        <ReadoutRow label="Queue" title={summary.detail}>
          {summary.oldestMs.toFixed(0)} ms
        </ReadoutRow>
      )}
      {overruns > 0 && (
        <ReadoutRow label="Drops" title="Samples lost since the radio opened">
          {overruns}
        </ReadoutRow>
      )}
    </Readout>
  );
}
