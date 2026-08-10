// The receiver node (CANVAS §1). The tuning dial is the signature element of the whole UI and it
// is the face of every device node; everything else the radio has is drawn from `Capabilities`
// alone, so a new device setting still needs zero frontend work (PLAN §6).
//
// Three states, each first-class (CANVAS §3): no radio named yet, and the node *is* the "open a
// radio" invitation; named and attached, and it is the instrument; named and absent, and it is
// the same node with dead controls and its wires kept — never silently rebound to whatever else
// is plugged in.
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { BTN, BTN_QUIET, CHIP, LABEL } from "../../components/controls";
import { tuningRange } from "../../components/dial";
import { DIAL_ID, FrequencyDial } from "../../components/FrequencyDial";
import { deviceId, ReceiverChoices } from "../../components/OpenRadio";
import { RadioSettings } from "../../components/RadioSettings";
import { createDeviceSet, STATE_KEY } from "../../lib/api";
import { pushToast } from "../../lib/toasts";
import type { DeviceInfo, DeviceRef, DeviceSet, PatchNode } from "../../lib/types";
import { useDevicePatch } from "../../lib/useDevicePatch";
import { deviceRefOf, refMatches, unboundChannels } from "../binding";
import { useStationContext } from "../context";
import { patchNode } from "../graph";
import { FaceBody, NodeShell } from "./NodeShell";

/** One dial per device node, and an id has to be unique in the document — this is the handle a
 * keyboard binding uses to reach the selected node's dial. */
export function deviceDialId(node: string): string {
  return `${DIAL_ID}:${node}`;
}

/** The radio a reference names, in the terms the operator would use to go and find it. A ref
 * carries a serial or a key, never both (CANVAS §3). */
export function refLabel(reference: DeviceRef): string {
  const identity = reference.serial ?? reference.key;
  return identity == null ? reference.backend : `${reference.backend} · ${identity}`;
}

/** A running scan drives the tuning itself, and the server refuses ours while it does
 * (PLAN §18). A faulted scan has already stopped, so the dial comes back with it. */
export function scannerOwnsTuning(set: DeviceSet): boolean {
  return set.scanner != null && set.scanner.error == null;
}

export function DeviceFace({ node }: { node: PatchNode }) {
  const station = useStationContext();
  const { applyPatch } = useDevicePatch();
  const queryClient = useQueryClient();
  // The kind test is what narrows `node.data` to a device node's payload.
  const reference = node.kind === "device" ? (node.data.device ?? null) : null;
  const set = station.devices.get(node.id) ?? null;

  const open = useMutation({
    mutationFn: createDeviceSet,
    // A radio that just arrived can be the one a channel node has been waiting for, and apply is
    // idempotent, so asking every time costs nothing.
    onSuccess: () => station.apply(),
    // Naming the radio has already flipped this face to its disconnected state by the time a
    // refusal lands, so the inline error below is no longer on screen to carry it.
    onError: (error: Error) => pushToast(error.message),
    onSettled: () => void queryClient.invalidateQueries({ queryKey: STATE_KEY }),
  });

  // The durable reference is what the patch stores — never an engine id, which is allocated per
  // run and would bind this node to whichever radio opened first (CANVAS §3).
  const nameRadio = (chosen: DeviceRef | null): void =>
    station.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, node.id, (stored) =>
        stored.kind === "device" ? { ...stored, data: { device: chosen } } : stored,
      ),
    }));

  const bind = (device: DeviceInfo): void => {
    const chosen = deviceRefOf(device);
    nameRadio(chosen);
    if (station.deviceSets.some((candidate) => refMatches(chosen, candidate.device))) {
      station.apply();
    } else {
      open.mutate(deviceId(device));
    }
  };

  if (reference === null) {
    return (
      <NodeShell node={node} title="Receiver" category="source" subtitle="no radio" live={false}>
        <FaceBody>
          <div className="flex flex-col gap-2 p-2">
            <span className={LABEL}>Open a receiver</span>
            <ReceiverChoices
              onChoose={bind}
              busy={open.isPending}
              error={open.error?.message ?? null}
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
        title="Receiver"
        category="source"
        subtitle="not attached"
        live={false}
      >
        <FaceBody>
          <div className="flex flex-col gap-2 p-2">
            <p className="text-sm text-ink-dim">
              Waiting for <span className="font-mono text-ink">{refLabel(reference)}</span>. Its
              wires stay drawn and nothing else will be bound here — plug it back in and this node
              picks it up.
            </p>
            <button
              type="button"
              className={`${BTN} self-start`}
              title="Unbind this node so it can name another radio"
              onClick={() => nameRadio(null)}
            >
              Forget this radio
            </button>
          </div>
        </FaceBody>
      </NodeShell>
    );
  }

  const scanning = scannerOwnsTuning(set);
  const overruns = set.overruns ?? 0;
  const orphans = unboundChannels(station.graph, node.id, set, station.channels);

  return (
    <NodeShell
      node={node}
      title={set.device.label}
      category="source"
      subtitle={<span className={set.status === "error" ? "text-danger" : ""}>{set.status}</span>}
    >
      <FaceBody>
        <div className="flex min-h-full flex-col">
          {/* The dial sizes off the node, not the viewport: this is the container its digits read
              (see `DialSize`). */}
          <div className="@container flex flex-col gap-1 border-b border-line p-2">
            <FrequencyDial
              id={deviceDialId(node.id)}
              size="face"
              hz={set.settings.center_hz ?? 0}
              range={tuningRange(set.capabilities)}
              disabled={scanning}
              onTune={(hz) => applyPatch(set.id, { center_hz: hz })}
            />
            {scanning && (
              <p className="text-xs text-ink-dim">
                The scanner is driving this radio; tuning from here is refused until it stops.
              </p>
            )}
          </div>

          <div className="p-2">
            <RadioSettings active={set} />
          </div>

          {(overruns > 0 || set.error != null) && (
            <div className="flex flex-wrap items-center gap-2 border-t border-line p-2">
              {overruns > 0 && (
                <span
                  className={CHIP}
                  title="Device samples dropped at the capture ring since the radio opened — the DSP thread is behind, and audio and spectrum have gaps"
                >
                  <span className="legend">Overruns</span>
                  {overruns}
                </span>
              )}
              {set.error != null && (
                <p role="alert" className="font-mono text-xs text-danger">
                  Device fault · {set.error}
                </p>
              )}
            </div>
          )}

          {orphans.length > 0 && (
            <div className="flex flex-col gap-1 border-t border-line p-2">
              <span className={LABEL}>Channels with no node</span>
              <span className="flex flex-wrap gap-1">
                {orphans.map((channel) => (
                  <span key={channel.id} className={CHIP}>
                    {channel.settings.params.type}
                  </span>
                ))}
              </span>
            </div>
          )}

          <div className="mt-auto flex justify-end border-t border-line p-2">
            <button
              type="button"
              className={BTN_QUIET}
              title="Unbind this node; the radio stays open and the wires stay drawn"
              onClick={() => nameRadio(null)}
            >
              Forget radio
            </button>
          </div>
        </div>
      </FaceBody>
    </NodeShell>
  );
}
