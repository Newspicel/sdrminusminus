// The device node. The tuning dial is the signature element of the whole UI and it is the face of
// every device node; everything else the radio has is drawn from `Capabilities` alone, so a new
// device setting still needs zero frontend work (PLAN §6).
//
// Three states, each first-class (CANVAS §3): no radio named yet, and the node *is* the "open a
// radio" invitation; named and attached, and it is the instrument; named and absent, and it is
// the same node with dead controls and its wires kept — never silently rebound to whatever else
// is plugged in.
//
// Its left side is what is done *to* the radio — a scanner's control wire, and the transmit input
// PLAN §12a reserves — while everything it produces leaves on the right.
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { BTN, BTN_QUIET, CHIP, LABEL } from "../../components/controls";
import { isTunable, tuningRange } from "../../components/dial";
import { DIAL_ID, FrequencyDial } from "../../components/FrequencyDial";
import { DeviceChoices, deviceId } from "../../components/OpenRadio";
import { PlaybackTransport } from "../../components/PlaybackTransport";
import { RadioSettings } from "../../components/RadioSettings";
import { createDeviceSet, STATE_KEY, stateQuery } from "../../lib/api";
import { pushToast } from "../../lib/toasts";
import type {
  Capabilities,
  DeviceInfo,
  DeviceRef,
  DeviceSet,
  DeviceSettings,
  PatchNode,
} from "../../lib/types";
import { forStream, useDevicePatch } from "../../lib/useDevicePatch";
import { deviceRefOf, refMatches, unboundChannels } from "../binding";
import { useWorkspaceContext } from "../context";
import { patchNode, rxStreamCount, streamLabel } from "../graph";
import { FaceBody, NodeShell, useFaceActive } from "./NodeShell";

/** An id has to be unique in the document. Stream 0's dial keeps the bare id — it is the one the
 * `f` keyboard binding reaches, and a single-stream radio only has that one. */
export function deviceDialId(node: string, stream = 0): string {
  return stream === 0 ? `${DIAL_ID}:${node}` : `${DIAL_ID}:${node}:${stream}`;
}

/** One dial's worth of the face: which stream it tunes, the IQ port it answers to (`null` when
 * the radio has one tuning for every lane and the single dial needs no name), and the centre it
 * shows — the lane's own override where one exists, the radio-wide value otherwise. */
export interface TunerDial {
  stream: number;
  port: string | null;
  hz: number;
}

/**
 * The dials this radio's face draws. One, unlabelled, unless the radio itself declares tuning
 * per-stream (`Capabilities::per_stream`): a coherent array shares one tuner by definition, so
 * even four lanes get a single dial — while a radio with a synthesizer per stream gets one per
 * lane, each named after the IQ port it feeds so the dial and the wire read as the same thing.
 */
export function tunerDials(set: DeviceSet): TunerDial[] {
  const capabilities = set.capabilities;
  const scope = capabilities.per_stream;
  const streams = rxStreamCount(capabilities);
  // One dial needs no name, and two named all but the first would read as if the unnamed one were
  // the radio's rather than lane 0's.
  if (scope?.tuning !== true || streams < 2) {
    return [{ stream: 0, port: null, hz: set.settings.center_hz ?? 0 }];
  }
  return Array.from({ length: streams }, (_, stream) => ({
    stream,
    port: streamLabel("iq", stream, streams),
    hz: forStream(set.settings, stream, scope).center_hz ?? 0,
  }));
}

/** The retune delta for one dial: a stream override on a radio whose lanes tune apart — so only
 * the lane touched moves — and the radio-wide centre everywhere else. */
export function tuneDelta(capabilities: Capabilities, stream: number, hz: number): DeviceSettings {
  return capabilities.per_stream?.tuning === true
    ? { streams: [{ stream, center_hz: hz }] }
    : { center_hz: hz };
}

/** The radio a reference names, in the terms the operator would use to go and find it. A ref
 * carries a serial or a key, never both (CANVAS §3). */
export function refLabel(reference: DeviceRef): string {
  const identity = reference.serial ?? reference.key;
  return identity == null ? reference.backend : `${reference.backend} · ${identity}`;
}

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

/** A running scan drives the tuning itself, and the server refuses ours while it does
 * (PLAN §18). A faulted scan has already stopped, so the dial comes back with it. */
export function scannerOwnsTuning(set: DeviceSet): boolean {
  return set.scanner != null && set.scanner.error == null;
}

export function DeviceFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const queryClient = useQueryClient();
  // The kind test is what narrows `node.data` to a device node's payload.
  const reference = node.kind === "device" ? (node.data.device ?? null) : null;
  const set = workspace.devices.get(node.id) ?? null;

  const open = useMutation({
    mutationFn: createDeviceSet,
    // A radio that just arrived can be the one a channel node has been waiting for, and apply is
    // idempotent, so asking every time costs nothing.
    onSuccess: () => workspace.apply(),
    // Naming the radio has already flipped this face to its disconnected state by the time a
    // refusal lands, so the inline error below is no longer on screen to carry it.
    onError: (error: Error) => pushToast(error.message),
    onSettled: () => void queryClient.invalidateQueries({ queryKey: STATE_KEY }),
  });

  // The durable reference is what the patch stores — never an engine id, which is allocated per
  // run and would bind this node to whichever radio opened first (CANVAS §3).
  const nameRadio = (chosen: DeviceRef | null): void =>
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, node.id, (stored) =>
        stored.kind === "device" ? { ...stored, data: { device: chosen } } : stored,
      ),
    }));

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
            <span className={LABEL}>Open a device</span>
            <DeviceChoices
              onChoose={bind}
              onAddNetwork={(id) => openNetwork.mutate(id)}
              busy={open.isPending || openNetwork.isPending}
              error={open.error?.message ?? openNetwork.error?.message ?? null}
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
  const orphans = unboundChannels(workspace.graph, node.id, set, workspace.channels);

  return (
    <NodeShell
      node={node}
      title={set.device.label}
      category="source"
      subtitle={<span className={set.status === "error" ? "text-danger" : ""}>{set.status}</span>}
    >
      <FaceBody>
        <div className="flex min-h-full flex-col">
          <Tuner node={node.id} set={set} scanning={scanning} />

          {set.playback != null && <PlaybackTransport set={set} status={set.playback} />}

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
