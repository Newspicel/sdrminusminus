// The terminal faces: what a decoded or demodulated stream ends up in (CANVAS §1). Each one
// fronts machinery that already exists — the audio engine, the map, the stored decoder log, its
// export, and the device recorder — so a wire into one of these is a subscription, never a new
// data path (CANVAS §2).
import { useMutation } from "@tanstack/react-query";
import { BTN, BTN_DANGER, CHIP, LABEL } from "../../components/controls";
import { DecoderLogPanel } from "../../components/DecoderLogPanel";
import { MapPanel } from "../../components/MapPanel";
import { ScannerPanel } from "../../components/ScannerPanel";
import { Slider } from "../../components/Slider";
import { decoderLogExportUrl, recordDeviceSet } from "../../lib/api";
import { useChannelAudio } from "../../lib/audio/useChannelAudio";
import { pushToast } from "../../lib/toasts";
import type { ChannelInfo, PatchNode } from "../../lib/types";
import { inputsOf } from "../binding";
import { deviceSetOf, useStationContext } from "../context";
import { FaceBody, FaceEmpty, NodeShell } from "./NodeShell";

/** One channel wired into a sink, resolved to the engine objects behind it. */
type Input = { node: string; deviceSet: number; channel: ChannelInfo };

function useInputs(node: string, port: string): Input[] {
  const station = useStationContext();
  return inputsOf(station.graph, node, port, station.devices, station.channels);
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

export function MapFace({ node }: { node: PatchNode }) {
  const inputs = useInputs(node.id, "events");
  return (
    <NodeShell
      node={node}
      title="Map"
      category="display"
      subtitle={inputs.length > 0 ? `${inputs.length} in` : undefined}
    >
      <FaceBody scroll={false}>
        {inputs.length === 0 ? (
          <FaceEmpty>Wire a decoder's events out to plot its positions.</FaceEmpty>
        ) : (
          <MapPanel className="h-full min-h-0 w-full flex-1" />
        )}
      </FaceBody>
    </NodeShell>
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
  const station = useStationContext();
  const inputs = useInputs(node.id, "events");
  const kinds = [
    ...new Set(
      inputs.flatMap((input) => {
        const type = input.channel.settings.params.type;
        return station.context.channelTypes.find((t) => t.type_id === type)?.decoder_kind ?? [];
      }),
    ),
  ];
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
  const record = useMutation({
    mutationFn: (action: "start" | "stop") => recordDeviceSet(set?.id ?? 0, action),
    onError: (error: Error) => pushToast(error.message),
  });
  const recording = set?.recording ?? null;
  return (
    <NodeShell
      node={node}
      title="Recorder"
      category="sink"
      subtitle={recording === null ? undefined : "recording"}
      live={set !== null}
    >
      <FaceBody>
        {set === null ? (
          <FaceEmpty>Wire a receiver's IQ out to record it.</FaceEmpty>
        ) : (
          <div className="flex flex-col gap-2 p-2">
            <button
              type="button"
              className={recording === null ? BTN : BTN_DANGER}
              disabled={record.isPending}
              onClick={() => record.mutate(recording === null ? "start" : "stop")}
            >
              {recording === null ? "Record" : "Stop"}
            </button>
            {recording !== null && (
              <>
                <span className={CHIP}>{recording.file}</span>
                <span className={CHIP}>
                  {(recording.bytes / 1e6).toFixed(1)} MB · {recording.overruns} drops
                </span>
                {recording.error != null && (
                  <p role="alert" className="text-xs text-danger">
                    {recording.error}
                  </p>
                )}
              </>
            )}
          </div>
        )}
      </FaceBody>
    </NodeShell>
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
