import { Collapsible } from "@base-ui/react/collapsible";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Radar } from "lucide-react";
import { Button } from "../../components/BaseControls";
import { BTN_PRIMARY, BTN_QUIET, ICON_BTN } from "../../components/controls";
import { DevOnly } from "../../components/DevOnly";
import { deviceId } from "../../components/devices";
import { inTuningRange, isTunable, tuningRange } from "../../components/dial";
import { dialId, FrequencyDial } from "../../components/FrequencyDial";
import { formatMhz } from "../../components/format";
import { Icon } from "../../components/Icon";
import { DeviceChoices } from "../../components/OpenRadio";
import { RadioSettings } from "../../components/RadioSettings";
import { Readout, ReadoutRow } from "../../components/Readout";
import { Tip } from "../../components/Tip";
import { TuneTo } from "../../components/TuneTo";
import { TuningLock } from "../../components/TuningLock";
import { createDeviceSet, devicesQuery, STATE_KEY, stateQuery } from "../../lib/api";
import { queueSummary, usePipelineHealth } from "../../lib/pipeline";
import { toastError } from "../../lib/toasts";
import type { DeviceInfo, DeviceRef, DeviceSet, PatchNode, PatchNodeOf } from "../../lib/types";
import { useDevicePatch } from "../../lib/useDevicePatch";
import { claimedDevices, deviceRefOf, refMatches } from "../binding";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { releaseRadio } from "../remove";
import { arrayHolding } from "./arrayNode";
import {
  autoTuning,
  faultSaid,
  type Hearing,
  hearing,
  lockStream,
  refLabel,
  tuneDelta,
  tunerDials,
  tuningDelta,
} from "./deviceNode";
import { FaceBody, FaceFooter, NodeShell, useFaceActive } from "./NodeShell";

type DeviceNodeData = PatchNodeOf<"device">["data"];

function AutoTuning({ set, stream }: { set: DeviceSet; stream: number }) {
  const { applyPatch } = useDevicePatch();
  const auto = autoTuning(set, stream);
  return (
    <Tip
      text={auto ? "Auto mode: following decoders" : "Auto mode: follow decoders"}
      render={
        <Button
          type="button"
          className={`${ICON_BTN} ${auto ? "bg-accent/15" : ""}`}
          aria-label={auto ? "Tune by hand" : "Follow the decoders"}
          aria-pressed={auto}
          onClick={() =>
            applyPatch(set.id, tuningDelta(set.capabilities, stream, auto ? "manual" : "auto"))
          }
        />
      }
    >
      <span className={auto ? "flex text-accent" : "flex"}>
        <Icon glyph={Radar} size={16} />
      </span>
    </Tip>
  );
}

function Tuner({
  node,
  set,
  lockedStreams,
  onLock,
  arrayTuning,
}: {
  node: string;
  set: DeviceSet;
  lockedStreams: readonly number[];
  onLock: (stream: number, locked: boolean) => void;
  arrayTuning: boolean;
}) {
  const { applyPatch } = useDevicePatch();
  const active = useFaceActive();
  const range = tuningRange(set.capabilities);
  const pinned = !isTunable(range);
  const tune = (stream: number, hz: number): void =>
    applyPatch(set.id, tuneDelta(set.capabilities, stream, hz));
  return (
    <div
      className="@container flex flex-col gap-1 border-b border-line p-2"
      title={
        arrayTuning
          ? "Tune the connected Array node to keep its member radios synchronized"
          : undefined
      }
    >
      {tunerDials(set).map((dial) => {
        const locked = lockedStreams.includes(dial.stream);
        const held = pinned || locked || arrayTuning;
        return (
          <div key={dial.stream} className="flex flex-col">
            {dial.port !== null && (
              <span className="legend text-[8px] leading-none">{dial.port}</span>
            )}
            <div className="flex min-w-0 items-center gap-2">
              <FrequencyDial
                id={dialId(node, dial.stream)}
                hz={dial.hz}
                range={range}
                disabled={held}
                wheelTunes={active}
                onTune={(hz) => tune(dial.stream, hz)}
              />
              <span className="ml-auto flex shrink-0 items-center gap-1">
                {!pinned && (
                  <>
                    <TuneTo
                      title={
                        dial.port === null
                          ? "Type a frequency"
                          : `Type a frequency for ${dial.port}`
                      }
                      hz={dial.hz}
                      hint={`Reaches ${formatMhz(range.min)} – ${formatMhz(range.max)}`}
                      resolve={(entered) => inTuningRange(entered, range)}
                      disabled={held}
                      onTune={(hz) => tune(dial.stream, hz)}
                    />
                    {!arrayTuning && <AutoTuning set={set} stream={dial.stream} />}
                    {!arrayTuning && (
                      <TuningLock
                        locked={locked}
                        held="Tuning locked"
                        free="Lock tuning"
                        onLock={(next) => onLock(dial.stream, next)}
                      />
                    )}
                  </>
                )}
              </span>
            </div>
          </div>
        );
      })}
    </div>
  );
}

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

  return (
    <NodeShell
      node={node}
      title={set.device.label}
      category="source"
      subtitle={<Heard set={set} />}
    >
      <FaceBody>
        <Tuner
          node={node.id}
          set={set}
          arrayTuning={arrayTuning}
          lockedStreams={lockedStreams}
          onLock={(stream, next) =>
            editNode({ locked_streams: lockStream(lockedStreams, stream, next) })
          }
        />

        <RadioSettings active={set} className="p-2" sampleRateLocked={arrayTuning} />

        <DevOnly>
          <DeviceHealth set={set} />
        </DevOnly>

        {set.error != null && <Fault set={set} />}
      </FaceBody>
      <FaceFooter>
        <Button
          type="button"
          className={BTN_QUIET}
          title="Close this radio and free the node — the device is released and the wires stay drawn"
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
  return (
    <>
      {summary !== null && (
        <span className="legend" title={summary.detail}>
          Queue {summary.oldestMs.toFixed(0)} ms
        </span>
      )}
      {overruns > 0 && (
        <Readout>
          <ReadoutRow
            label="Drops"
            title="Samples lost during capture since the radio opened, including reported device gaps, full queues, and stale samples."
          >
            {overruns}
          </ReadoutRow>
        </Readout>
      )}
    </>
  );
}
