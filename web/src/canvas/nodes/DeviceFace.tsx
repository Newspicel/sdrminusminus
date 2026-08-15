import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Button } from "../../components/BaseControls";
import { BTN, BTN_QUIET } from "../../components/controls";
import { deviceId } from "../../components/devices";
import { isTunable, tuningRange } from "../../components/dial";
import { FrequencyDial } from "../../components/FrequencyDial";
import { DeviceChoices } from "../../components/OpenRadio";
import { PlaybackTransport } from "../../components/PlaybackTransport";
import { RadioSettings } from "../../components/RadioSettings";
import { Readout, ReadoutRow } from "../../components/Readout";
import { createDeviceSet, STATE_KEY, stateQuery } from "../../lib/api";
import { pushToast } from "../../lib/toasts";
import type { DeviceInfo, DeviceRef, DeviceSet, PatchNode } from "../../lib/types";
import { useDevicePatch } from "../../lib/useDevicePatch";
import { claimedDevices, deviceRefOf, refMatches, unboundChannels } from "../binding";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { releaseRadio } from "../remove";
import { deviceDialId, refLabel, scannerOwnsTuning, tuneDelta, tunerDials } from "./deviceNode";
import { FaceBody, FaceFooter, NodeShell, useFaceActive } from "./NodeShell";

/** Its own component so it can read whether this face is the active one: the dial's wheel belongs
 * to the camera until the node is clicked (`useFaceActive`), or one notch would tune the radio and
 * pan the patch at once. It is also where the dial's `@container` sits — the digits size off the
 * node, never off the viewport (see `DIGIT_SIZE`). */
function Tuner({ node, set, scanning }: { node: string; set: DeviceSet; scanning: boolean }) {
  const { applyPatch } = useDevicePatch();
  const active = useFaceActive();
  const range = tuningRange(set.capabilities);
  const pinned = !isTunable(range);
  return (
    <div className="@container flex flex-col gap-1 border-b border-line p-2">
      {tunerDials(set).map((dial) => (
        <div key={dial.stream} className="flex flex-col">
          {/* The `legend` class uppercases, so the label renders exactly as the port handle
              beside it does: `iq2` on the wire is IQ2 on the dial. */}
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

export function DeviceFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const queryClient = useQueryClient();
  const reference = node.kind === "device" ? (node.data.device ?? null) : null;
  const set = workspace.devices.get(node.id) ?? null;

  const open = useMutation({
    mutationFn: createDeviceSet,
    onSuccess: () => workspace.apply(),
    // Naming the radio has already flipped this face to its disconnected state by the time a
    // refusal lands, so the inline error below is no longer on screen to carry it.
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

  // Letting go of a radio closes it: leaving it open would keep the USB device claimed — no
  // other app, and no other node, can have it — with nothing on the canvas pointing at it. The
  // node and its wires stay, and this node's device settings are stored, so naming a radio here
  // again reopens it exactly as it was.
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

  // A network radio is named rather than discovered, so unlike `bind` there is no probe result to
  // name the node from — and the address that was typed is not necessarily the key the server will
  // probe it under, because it canonicalizes the endpoint (a defaulted port, a lower-cased host).
  // Opening it first and reading the device back off the set it created is what keeps the stored
  // reference and the probe list the same string.
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
      <NodeShell node={node} title="Device" category="source" subtitle="not attached" live={false}>
        <FaceBody>
          <p className="p-3 text-sm text-ink-dim">
            Waiting for <span className="font-mono text-ink">{refLabel(reference)}</span>. Its wires
            stay drawn and nothing else will be bound here — plug it back in and this node picks it
            up.
          </p>
        </FaceBody>
        <FaceFooter>
          <Button
            type="button"
            className={BTN}
            title="Unbind this node so it can name another radio"
            onClick={() => forget.mutate()}
            disabled={forget.isPending}
          >
            Forget radio
          </Button>
        </FaceFooter>
      </NodeShell>
    );
  }

  const scanning = scannerOwnsTuning(set);
  const overruns = set.overruns ?? 0;
  const orphans = unboundChannels(workspace.graph, node.id, set, workspace.channels);

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

        {(overruns > 0 || orphans.length > 0) && (
          <Readout>
            {overruns > 0 && (
              <ReadoutRow
                label="Overruns"
                title="Device samples dropped at the capture ring since the radio opened — the DSP thread is behind, and audio and spectrum have gaps"
              >
                {overruns}
              </ReadoutRow>
            )}
            {orphans.length > 0 && (
              <ReadoutRow label="Channels with no node">
                {orphans.map((channel) => channel.settings.params.type).join(" · ")}
              </ReadoutRow>
            )}
          </Readout>
        )}

        {set.error != null && (
          <p role="alert" className="border-t border-line p-2 font-mono text-xs text-danger">
            Device fault · {set.error}
          </p>
        )}
      </FaceBody>
      <FaceFooter>
        <Button
          type="button"
          className={BTN_QUIET}
          title="Close this radio and unbind the node — the USB device is released and the wires stay drawn"
          onClick={() => forget.mutate()}
          disabled={forget.isPending}
        >
          {forget.isPending ? "Closing…" : "Forget radio"}
        </Button>
      </FaceFooter>
    </NodeShell>
  );
}
