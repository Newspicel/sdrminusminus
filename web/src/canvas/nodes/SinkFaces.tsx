// The terminal faces: what a decoded or demodulated stream ends up in (CANVAS §1). Each one
// fronts machinery that already exists — the audio engine, the map, the stored decoder log, its
// export, and the device recorder — so a wire into one of these is a subscription, never a new
// data path (CANVAS §2).
import { useMutation } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { BTN, BTN_DANGER, CHIP, LABEL } from "../../components/controls";
import { DecoderLogPanel } from "../../components/DecoderLogPanel";
import { MapPanel } from "../../components/MapPanel";
import {
  deriveRecordControl,
  formatBytes,
  formatDuration,
  recordingElapsedS,
} from "../../components/recordings";
import { ScannerPanel } from "../../components/ScannerPanel";
import { Slider } from "../../components/Slider";
import { decoderLogExportUrl, recordDeviceSet } from "../../lib/api";
import { useChannelAudio } from "../../lib/audio/useChannelAudio";
import { type MapKind, mapKindsOf, referencePositions } from "../../lib/map/layers";
import { pushToast } from "../../lib/toasts";
import type {
  ChannelInfo,
  DeviceSet,
  PatchNode,
  RecordAction,
  RecordingStatus,
} from "../../lib/types";
import { inputsOf } from "../binding";
import { deviceSetOf, useStationContext } from "../context";
import { FaceBody, FaceEmpty, NodeShell, useFaceActive } from "./NodeShell";

/** One channel wired into a sink, resolved to the engine objects behind it. */
type Input = { node: string; deviceSet: number; channel: ChannelInfo };

function useInputs(node: string, port: string): Input[] {
  const station = useStationContext();
  return inputsOf(station.graph, node, port, station.devices, station.channels);
}

/** What the decoders wired into a sink emit. The wire is the filter, which is the whole reason
 * the map and the export are nodes rather than menu items (CANVAS §1). */
function useWiredKinds(inputs: readonly Input[]): string[] {
  const station = useStationContext();
  return [
    ...new Set(
      inputs.flatMap((input) => {
        const type = input.channel.settings.params.type;
        return station.context.channelTypes.find((t) => t.type_id === type)?.decoder_kind ?? [];
      }),
    ),
  ];
}

/** Client-side mixing (PLAN §9): the server ships one stream per channel and the browser adds
 * them up, so N listeners on one channel still cost one encode. */
export function SpeakerFace({ node }: { node: PatchNode }) {
  const inputs = useInputs(node.id, "audio");
  return (
    <NodeShell
      node={node}
      title="Speaker"
      category="sink"
      subtitle={inputs.length > 0 ? `${inputs.length} in` : undefined}
      live={inputs.length > 0}
    >
      <FaceBody>
        {inputs.length === 0 ? (
          <FaceEmpty>Wire a channel's audio out to this speaker.</FaceEmpty>
        ) : (
          inputs.map((input) => <AudioInput key={input.node} input={input} />)
        )}
      </FaceBody>
    </NodeShell>
  );
}

function AudioInput({ input }: { input: Input }) {
  const station = useStationContext();
  const audio = useChannelAudio(station.socket, input.deviceSet, input.channel.id);
  const label = station.graph.nodes.find((n) => n.id === input.node)?.label;
  return (
    <div className="flex flex-col gap-1 border-b border-line p-2 last:border-b-0">
      <div className="flex items-center gap-2">
        <button
          type="button"
          className={audio.playing ? BTN_DANGER : BTN}
          onClick={() => {
            // iOS resumes output only inside a gesture, so the click does both.
            audio.resumeOutput();
            if (audio.playing) {
              audio.stop();
            } else {
              audio.start();
            }
          }}
        >
          {audio.playing ? "Stop" : audio.pending ? "…" : "Play"}
        </button>
        <span className="legend truncate">
          {label ?? input.channel.settings.params.type.toUpperCase()}
        </span>
      </div>
      <Slider
        label="Volume"
        value={audio.volume}
        min={0}
        max={1}
        step={0.01}
        onChange={audio.setVolume}
      />
      {audio.suspended && (
        <button type="button" className={BTN} onClick={audio.resumeOutput}>
          Audio is suspended — click to resume
        </button>
      )}
      {audio.error !== null && (
        <p role="alert" className="text-xs text-danger">
          {audio.error}
        </p>
      )}
    </div>
  );
}

/** One layer per connected decoder (CANVAS §1) — the wires, not the store, decide what this map
 * plots, so two map nodes on different decoders show different things. */
export function MapFace({ node }: { node: PatchNode }) {
  const inputs = useInputs(node.id, "events");
  const wired = useWiredKinds(inputs);
  const kinds = mapKindsOf(wired);
  return (
    <NodeShell
      node={node}
      title="Map"
      category="display"
      subtitle={inputs.length > 0 ? `${inputs.length} in` : undefined}
      live={kinds.length > 0}
    >
      <FaceBody scroll={false}>
        {inputs.length === 0 ? (
          <FaceEmpty>Wire a decoder's events out to plot its positions.</FaceEmpty>
        ) : kinds.length === 0 ? (
          <FaceEmpty>
            Nothing wired in reports a position. ADS-B, AIS and APRS do; the rest have nowhere to be
            drawn.
          </FaceEmpty>
        ) : (
          <Plot
            kinds={kinds}
            references={referencePositions(inputs.map((input) => input.channel.settings.params))}
          />
        )}
      </FaceBody>
    </NodeShell>
  );
}

/** Its own component so it can read whether this face is the active one — the hook only answers
 * inside the shell, and the map has to give its wheel back to the camera until then. */
function Plot({
  kinds,
  references,
}: {
  kinds: readonly MapKind[];
  references: readonly (readonly [number, number])[];
}) {
  return (
    <MapPanel
      kinds={kinds}
      references={references}
      active={useFaceActive()}
      className="h-full min-h-0 w-full flex-1"
    />
  );
}

export function DecoderLogFace({ node }: { node: PatchNode }) {
  const station = useStationContext();
  const inputs = useInputs(node.id, "events");
  return (
    <NodeShell
      node={node}
      title="Decoder log"
      category="display"
      subtitle={inputs.length > 0 ? `${inputs.length} in` : undefined}
    >
      <FaceBody scroll={false}>
        <DecoderLogPanel deviceSets={station.deviceSets} />
      </FaceBody>
    </NodeShell>
  );
}

/** Fronts the decoder-log export API, filtered to the decoders wired into it — the wire is the
 * filter, which is the whole reason this is a node rather than a menu item. */
export function ExportFace({ node }: { node: PatchNode }) {
  const inputs = useInputs(node.id, "events");
  const kinds = useWiredKinds(inputs);
  return (
    <NodeShell
      node={node}
      title="Export"
      category="sink"
      subtitle={kinds.length > 0 ? kinds.join(" · ") : undefined}
      live={inputs.length > 0}
    >
      <FaceBody>
        {inputs.length === 0 ? (
          <FaceEmpty>Wire decoders in; their stored rows are what gets exported.</FaceEmpty>
        ) : (
          <div className="flex flex-col gap-2 p-2">
            <span className={LABEL}>Stored rows</span>
            <div className="flex gap-2">
              {(["csv", "json"] as const).map((format) => (
                <a
                  key={format}
                  className={BTN}
                  href={decoderLogExportUrl(format, kinds.length === 1 ? { kind: kinds[0] } : {})}
                  download
                >
                  {format.toUpperCase()}
                </a>
              ))}
            </div>
            {kinds.length > 1 && (
              <p className="text-xs text-ink-dim">
                Several decoders are wired in, and the export filter takes one kind — this exports
                the whole log.
              </p>
            )}
          </div>
        )}
      </FaceBody>
    </NodeShell>
  );
}

/** The device-level SigMF recorder (PLAN §5), drawn as the sink it is. */
export function RecorderFace({ node }: { node: PatchNode }) {
  const station = useStationContext();
  const set = deviceSetOf(station, node.id);
  return (
    <NodeShell
      node={node}
      title="Recorder"
      category="sink"
      subtitle={set?.recording == null ? undefined : "recording"}
      live={set !== null}
    >
      <FaceBody>
        {set === null ? (
          <FaceEmpty>Wire a receiver's IQ out to record it.</FaceEmpty>
        ) : (
          <RecordControl set={set} />
        )}
      </FaceBody>
    </NodeShell>
  );
}

/** `deriveRecordControl` owns the two rules this face must not restate: a start needs a running
 * receiver, and a faulted recording still reads as recording until it is explicitly stopped. */
function RecordControl({ set }: { set: DeviceSet }) {
  const record = useMutation({
    mutationFn: (action: RecordAction) => recordDeviceSet(set.id, action),
    onError: (error: Error) => pushToast(error.message),
  });
  const control = deriveRecordControl(set);
  if (control.kind === "idle") {
    return (
      <div className="flex flex-col gap-2 p-2">
        <button
          type="button"
          className={BTN}
          disabled={!control.canStart || record.isPending}
          title={control.canStart ? "Record IQ to a SigMF pair" : "The receiver must be running"}
          onClick={() => record.mutate("start")}
        >
          <span aria-hidden className="text-danger">
            ●
          </span>
          Record
        </button>
      </div>
    );
  }
  const status = control.status;
  return (
    <div className="flex flex-col gap-2 p-2">
      <button
        type="button"
        className={BTN_DANGER}
        disabled={record.isPending}
        onClick={() => record.mutate("stop")}
      >
        Stop
      </button>
      <RecordingReadout status={status} sampleRate={set.settings.sample_rate ?? 0} />
      <span className={CHIP}>{status.file}</span>
      {status.error != null && (
        <p role="alert" className="text-xs text-danger">
          {status.error}
        </p>
      )}
    </div>
  );
}

/** Its own component so the one-second tick re-renders the readout and not the whole face. Once
 * the recording faulted the writer is dead, so the tick stops with it — wall clock would
 * overstate what was captured. */
function RecordingReadout({ status, sampleRate }: { status: RecordingStatus; sampleRate: number }) {
  const faulted = status.error != null;
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (faulted) {
      return;
    }
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [faulted]);
  return (
    <span className={CHIP}>
      {formatDuration(recordingElapsedS(status, now, sampleRate))} · {formatBytes(status.bytes)}
      {status.overruns > 0 && ` · ${status.overruns} drops`}
    </span>
  );
}

/**
 * The scanner owns a receiver's tuning while it runs, and CANVAS §9 left where it lives open.
 * It is a node wired to the radio it drives: the edge *is* the ownership, which is the only way
 * to see at a glance which radio a running sweep has taken over — and client retunes on that
 * radio are refused while it does (PLAN §18), which the face says in words.
 */
export function ScannerFace({ node }: { node: PatchNode }) {
  const station = useStationContext();
  const set = deviceSetOf(station, node.id);
  // `DeviceSet.scanner` is absent when no sweep is running, so its presence *is* the ownership.
  const scanning = set?.scanner != null;
  return (
    <NodeShell
      node={node}
      title="Scanner"
      category="feature"
      subtitle={scanning ? "owns this radio" : undefined}
      live={set !== null}
    >
      <FaceBody>
        {set === null ? (
          <FaceEmpty>Wire a receiver in; the scanner drives its tuning.</FaceEmpty>
        ) : (
          <ScannerPanel active={set} />
        )}
      </FaceBody>
    </NodeShell>
  );
}
