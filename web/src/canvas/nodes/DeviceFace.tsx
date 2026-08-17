import { Collapsible } from "@base-ui/react/collapsible";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Button } from "../../components/BaseControls";
import { BTN_PRIMARY, BTN_QUIET } from "../../components/controls";
import { deviceId } from "../../components/devices";
import { isTunable, tuningRange } from "../../components/dial";
import { FrequencyDial } from "../../components/FrequencyDial";
import { DeviceChoices } from "../../components/OpenRadio";
import { PlaybackTransport } from "../../components/PlaybackTransport";
import { RadioSettings } from "../../components/RadioSettings";
import { Readout, ReadoutRow } from "../../components/Readout";
import { createDeviceSet, devicesQuery, STATE_KEY, stateQuery } from "../../lib/api";
import { pushToast } from "../../lib/toasts";
import type { DeviceInfo, DeviceRef, DeviceSet, PatchNode } from "../../lib/types";
import { useDevicePatch } from "../../lib/useDevicePatch";
import { claimedDevices, deviceRefOf, refMatches } from "../binding";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { releaseRadio } from "../remove";
import {
  deviceDialId,
  faultSaid,
  refLabel,
  scannerOwnsTuning,
  tuneDelta,
  tunerDials,
} from "./deviceNode";
import { FaceBody, FaceFooter, NodeShell, useFaceActive } from "./NodeShell";

function Tuner({ node, set, scanning }: { node: string; set: DeviceSet; scanning: boolean }) {
  const { applyPatch } = useDevicePatch();
  const active = useFaceActive();
  const range = tuningRange(set.capabilities);
  const pinned = !isTunable(range);
  return (
    <div className="@container flex flex-col gap-1 border-b border-line p-2">
      {tunerDials(set).map((dial) => (
        <div key={dial.stream} className="flex flex-col">
          {dial.port !== null && <span className="legend">{dial.port}</span>}
          <FrequencyDial
            id={deviceDialId(node, dial.stream)}
            hz={dial.hz}
            range={range}
            disabled={scanning || pinned}
            wheelTunes={active}
            onTune={(hz) => applyPatch(set.id, tuneDelta(set.capabilities, dial.stream, hz))}
          />
        </div>
      ))}
      {scanning && (
        <p className="text-xs text-ink-dim">
          The scanner is driving this radio; tuning from here is refused until it stops.
        </p>
      )}
    </div>
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
  const set = workspace.devices.get(node.id) ?? null;
  const onBus =
    reference !== null &&
    (attached.data?.devices ?? []).some((device) => refMatches(reference, device));

  const open = useMutation({
    mutationFn: createDeviceSet,
    onSuccess: () => workspace.apply(),
    onError: (error: Error) => pushToast(error.message),
    onSettled: () => void queryClient.invalidateQueries({ queryKey: STATE_KEY }),
  });

  const nameRadio = (chosen: DeviceRef | null): void =>
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, node.id, (stored) =>
        stored.kind === "device" ? { ...stored, data: { device: chosen } } : stored,
      ),
    }));

  const forget = useMutation({
    mutationFn: () => releaseRadio(workspace, node.id, () => nameRadio(null)),
    onError: (error: Error) => pushToast(error.message),
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
    onError: (error: Error) => pushToast(error.message),
    onSettled: () => void queryClient.invalidateQueries({ queryKey: STATE_KEY }),
  });

  if (reference === null) {
    return (
      <NodeShell node={node} title="Device" category="source" subtitle="no radio" live={false}>
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
        live={false}
      >
        <FaceBody>
          <p className="p-3 text-sm text-ink-dim">
            <span className="font-mono text-ink">{refLabel(reference)}</span>{" "}
            {onBus
              ? "is plugged in but not open. Open it to start the channels wired to this node."
              : "is not connected. Plug it back in and open it here — the wires and settings on this node are kept until then."}
          </p>
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

  const scanning = scannerOwnsTuning(set);
  const overruns = set.overruns ?? 0;

  return (
    <NodeShell
      node={node}
      title={set.device.label}
      category="source"
      subtitle={<span className={set.status === "error" ? "text-danger" : ""}>{set.status}</span>}
    >
      <FaceBody>
        <Tuner node={node.id} set={set} scanning={scanning} />

        {set.playback != null && <PlaybackTransport set={set} status={set.playback} />}

        <RadioSettings active={set} className="p-2" />

        {overruns > 0 && (
          <Readout>
            <ReadoutRow
              label="Drops"
              title="Device samples dropped at the capture ring since the radio opened — the DSP thread is behind, and audio and spectrum have gaps"
            >
              {overruns}
            </ReadoutRow>
          </Readout>
        )}

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
