import { Collapsible } from "@base-ui/react/collapsible";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link2, Radar } from "lucide-react";
import type { ReactNode } from "react";
import { Button } from "../../components/BaseControls";
import { BTN_PRIMARY, BTN_QUIET, ICON_BTN } from "../../components/controls";
import { deviceId } from "../../components/devices";
import { inTuningRange, isTunable, tuningRange } from "../../components/dial";
import { dialId, FrequencyDial } from "../../components/FrequencyDial";
import { DROPS_HINT, formatCount, formatMhz } from "../../components/format";
import { Icon } from "../../components/Icon";
import { laneLayout } from "../../components/laneRows";
import { DeviceChoices } from "../../components/OpenRadio";
import { RadioSettings } from "../../components/RadioSettings";
import { Tip } from "../../components/Tip";
import { TuneTo } from "../../components/TuneTo";
import { TuningLock } from "../../components/TuningLock";
import { createDeviceSet, devicesQuery, STATE_KEY, stateQuery } from "../../lib/api";
import { queueSummary, usePipelineHealth } from "../../lib/pipeline";
import { toastError } from "../../lib/toasts";
import type { DeviceInfo, DeviceRef, DeviceSet, PatchNode, PatchNodeOf } from "../../lib/types";
import { useRadioTune } from "../../lib/useRadioTune";
import { claimedDevices, deviceRefOf, refMatches } from "../binding";
import { useWorkspaceContext } from "../context";
import { patchNode, rxStreamCount, streamPort } from "../graph";
import { releaseRadio } from "../remove";
import { arrayHolding } from "./arrayNode";
import {
  allAutoTuning,
  allLocked,
  autoTuning,
  bondSaid,
  clippingSaid,
  coherentLanes,
  faultSaid,
  type Hearing,
  hearing,
  lanesMerged,
  lockAll,
  lockStream,
  lossSaid,
  refLabel,
  refusalSaid,
  type TunerDial,
  tunerDials,
} from "./deviceNode";
import { FaceBody, FaceFooter, NodeShell, useFaceActive } from "./NodeShell";

type DeviceNodeData = PatchNodeOf<"device">["data"];

function AutoTuning({ set, stream }: { set: DeviceSet; stream: number | "all" }) {
  const { setTuning, setTuningAll } = useRadioTune();
  const auto = stream === "all" ? allAutoTuning(set) : autoTuning(set, stream);
  const flip = (): void => {
    const next = auto ? "manual" : "auto";
    if (stream === "all") {
      setTuningAll(set, next);
    } else {
      setTuning(set, stream, next);
    }
  };
  return (
    <Tip
      text={auto ? "Auto mode: following decoders" : "Auto mode: follow decoders"}
      render={
        <Button
          type="button"
          className={`${ICON_BTN} ${auto ? "bg-accent/15" : ""}`}
          aria-label={auto ? "Tune by hand" : "Follow the decoders"}
          aria-pressed={auto}
          onClick={flip}
        />
      }
    >
      <span className={auto ? "flex text-accent" : "flex"}>
        <Icon glyph={Radar} size={16} />
      </span>
    </Tip>
  );
}

function LinkLanes({ linked, onLink }: { linked: boolean; onLink: (linked: boolean) => void }) {
  return (
    <Tip
      text={linked ? "Lanes tune together. Click to tune each lane" : "Tune all lanes together"}
      render={
        <Button
          type="button"
          className={`${ICON_BTN} ${linked ? "bg-accent/15" : ""}`}
          aria-label={linked ? "Tune lanes one by one" : "Tune all lanes together"}
          aria-pressed={linked}
          onClick={() => onLink(!linked)}
        />
      }
    >
      <span className={linked ? "flex text-accent" : "flex"}>
        <Icon glyph={Link2} size={16} />
      </span>
    </Tip>
  );
}

interface TunerProps {
  node: string;
  set: DeviceSet;
  lockedStreams: readonly number[];
  onLock: (locked: number[]) => void;
  split: boolean;
  onSplit: (split: boolean) => void;
  arrayTuning: boolean;
}

function DialRow({
  node,
  set,
  dial,
  locked,
  onLock,
  tools,
  onTune,
}: {
  node: string;
  set: DeviceSet;
  dial: TunerDial;
  locked: boolean;
  onLock: (locked: boolean) => void;
  tools: ReactNode;
  onTune: (hz: number) => void;
}) {
  const active = useFaceActive();
  const range = tuningRange(set.capabilities);
  const pinned = !isTunable(range);
  const held = pinned || locked;
  return (
    <div className="@container flex min-w-0 items-center gap-2">
      <FrequencyDial
        id={dialId(node, dial.stream)}
        hz={dial.hz}
        range={range}
        disabled={held}
        wheelTunes={active}
        onTune={onTune}
      />
      <span className="ml-auto flex shrink-0 items-center gap-1">
        {!pinned && (
          <TuneTo
            title="Type a frequency"
            hz={dial.hz}
            hint={`Reaches ${formatMhz(range.min)} to ${formatMhz(range.max)}`}
            resolve={(entered) => inTuningRange(entered, range)}
            disabled={held}
            onTune={onTune}
          />
        )}
        {!pinned && tools}
        {!pinned && (
          <TuningLock locked={locked} held="Tuning locked" free="Lock tuning" onLock={onLock} />
        )}
      </span>
    </div>
  );
}

function LaneDial({
  node,
  set,
  dial,
  locked,
}: {
  node: string;
  set: DeviceSet;
  dial: TunerDial;
  locked: boolean;
}) {
  const { tuneRadio } = useRadioTune();
  const active = useFaceActive();
  const range = tuningRange(set.capabilities);
  const held = !isTunable(range) || locked;
  const tune = (hz: number): void => tuneRadio(set, dial.stream, hz);
  return (
    <div className="flex h-7 items-center gap-2">
      <span className="w-14 shrink-0 truncate font-mono text-[11px] text-port-iq">{dial.port}</span>
      <div className="@container w-52 min-w-0">
        <FrequencyDial
          id={dialId(node, dial.stream)}
          hz={dial.hz}
          range={range}
          disabled={held}
          wheelTunes={active}
          onTune={tune}
        />
      </div>
      <span className="ml-auto flex shrink-0 items-center gap-1">
        <TuneTo
          title={`Type a frequency for ${dial.port}`}
          hz={dial.hz}
          hint={`Reaches ${formatMhz(range.min)} to ${formatMhz(range.max)}`}
          resolve={(entered) => inTuningRange(entered, range)}
          disabled={held}
          onTune={tune}
        />
      </span>
    </div>
  );
}

function Tuner(props: TunerProps) {
  const { node, set, lockedStreams, onLock, split, onSplit, arrayTuning } = props;
  const { tuneRadio, tuneAll } = useRadioTune();
  const caps = set.capabilities;
  const merged = lanesMerged(set);
  const dial = tunerDials(set)[0];
  const locked = merged ? allLocked(lockedStreams, caps) : lockedStreams.includes(0);
  const lock = (held: boolean): void =>
    onLock(merged ? lockAll(caps, held) : lockStream(lockedStreams, 0, held));
  const title = arrayTuning ? ARRAY_TUNED : undefined;
  const link = merged && (
    <LinkLanes
      linked={!split}
      onLink={(linked) => {
        onSplit(!linked);
        if (linked && dial !== undefined) {
          tuneAll(set, dial.hz);
        }
      }}
    />
  );
  if (merged && split) {
    return (
      <div className="flex h-7 items-center gap-2" title={title}>
        <span className="legend">Per lane</span>
        <span className="ml-auto flex shrink-0 items-center gap-1">
          {link}
          <AutoTuning set={set} stream="all" />
          <TuningLock locked={locked} held="Tuning locked" free="Lock tuning" onLock={lock} />
        </span>
      </div>
    );
  }
  if (dial === undefined) {
    return null;
  }
  return (
    <div title={title}>
      <DialRow
        node={node}
        set={set}
        dial={dial}
        locked={locked}
        onLock={lock}
        onTune={(hz) => (merged ? tuneAll(set, hz) : tuneRadio(set, 0, hz))}
        tools={
          <>
            {link}
            <AutoTuning set={set} stream={merged ? "all" : 0} />
          </>
        }
      />
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
  const split = node.kind === "device" && node.data.split_tuning === true;
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
  const merged = lanesMerged(set);
  const dials = tunerDials(set);
  const streams = rxStreamCount(set.capabilities);
  const rowPerPort = laneLayout(set.capabilities).lanes === streams;

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
          advised={advised}
          lead={
            <Tuner
              node={node.id}
              set={set}
              arrayTuning={arrayTuning}
              lockedStreams={lockedStreams}
              onLock={(locked_streams) => editNode({ locked_streams })}
              split={split}
              onSplit={(split_tuning) => editNode({ split_tuning })}
            />
          }
          laneLeads={
            merged && split
              ? dials.map((dial) => (
                  <LaneDial
                    key={dial.stream}
                    node={node.id}
                    set={set}
                    dial={dial}
                    locked={lockedStreams.includes(dial.stream)}
                  />
                ))
              : undefined
          }
          ports={
            rowPerPort
              ? Array.from({ length: streams }, (_, stream) => streamPort("iq", stream))
              : undefined
          }
        />

        {set.error != null && <Fault set={set} />}
        <Refused set={set} />
      </FaceBody>
      <FaceFooter>
        <DeviceHealth set={set} />
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

const LOSS_HINT =
  "The radio sends more than its link or this computer carries. Lower the rate or the lanes";

function Stat({
  label,
  title,
  tone = "",
  children,
}: {
  label: string;
  title: string;
  tone?: string;
  children: ReactNode;
}) {
  return (
    <span
      className="inline-flex items-center gap-1 font-mono text-[11px] whitespace-nowrap text-ink-faint"
      title={title}
    >
      {label} <b className={`font-medium ${tone === "" ? "text-ink" : tone}`}>{children}</b>
    </span>
  );
}

function DeviceHealth({ set }: { set: DeviceSet }) {
  const health = usePipelineHealth((state) => state.health);
  const summary = queueSummary(health, set.id);
  const overruns = set.overruns ?? 0;
  const clipping = clippingSaid(set);
  const loss = lossSaid(set);
  const bond = lanesMerged(set) ? bondSaid(set.capabilities.coherence) : null;
  return (
    <span className="mr-auto flex min-w-0 flex-wrap items-center gap-3">
      {summary !== null && (
        <Stat label="Queue" title={summary.detail}>
          {summary.oldestMs.toFixed(0)} ms
        </Stat>
      )}
      {loss !== null && (
        <Stat label="Lost" title={LOSS_HINT} tone="text-warn">
          {loss}
        </Stat>
      )}
      {overruns > 0 && (
        <Stat label="Drops" title={DROPS_HINT} tone="text-warn">
          {formatCount(overruns)}
        </Stat>
      )}
      {clipping !== null && (
        <Stat label="Clipping" title="The ADC is at full scale. Lower the gain" tone="text-danger">
          {clipping}
        </Stat>
      )}
      {bond !== null && (
        <span
          className="inline-flex items-center gap-1 font-mono text-[11px] whitespace-nowrap text-ink-faint"
          title="The lanes sample on one clock, so their streams line up in time"
        >
          <Icon glyph={Link2} size={12} />
          {bond}
        </span>
      )}
    </span>
  );
}
