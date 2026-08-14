import { useMutation } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { BTN, BTN_DANGER, CHIP, LABEL } from "../../components/controls";
import { DecoderLogPanel } from "../../components/DecoderLogPanel";
import { DecoderView, hasDecoderView } from "../../components/DecoderPanels";
import type { WireScope } from "../../components/decoderLog";
import { MapPanel } from "../../components/MapPanel";
import {
  deriveRecordControl,
  formatBytes,
  formatDuration,
  recordingElapsedS,
} from "../../components/recordings";
import { ScannerPanel } from "../../components/ScannerPanel";
import { Slider } from "../../components/Slider";
import { VideoView } from "../../components/VideoView";
import { callAudioUrl, decoderLogExportUrl, recordDeviceSet } from "../../lib/api";
import { useChannelAudio } from "../../lib/audio/useChannelAudio";
import { type MapKind, mapKindsOf, referencePositions } from "../../lib/map/layers";
import { pushToast } from "../../lib/toasts";
import type {
  ChannelInfo,
  DeviceSet,
  PatchNode,
  RecordAction,
  RecordingStatus,
  VoiceCall,
} from "../../lib/types";
import { inputsOf, iqSourceOf } from "../binding";
import { deviceSetOf, useWorkspaceContext } from "../context";
import { FaceBody, FaceEmpty, NodeShell, useFaceActive } from "./NodeShell";

/** One channel wired into a sink, resolved to the engine objects behind it. */
type Input = { node: string; deviceSet: number; channel: ChannelInfo };

function useInputs(node: string, port: string): Input[] {
  const workspace = useWorkspaceContext();
  return inputsOf(workspace.graph, node, port, workspace.devices, workspace.channels);
}

function useWiredDecoders(inputs: readonly Input[]): { input: Input; kind: string }[] {
  const workspace = useWorkspaceContext();
  return inputs.flatMap((input) => {
    const type = input.channel.settings.params.type;
    const kind = workspace.context.channelTypes.find((t) => t.type_id === type)?.decoder_kind;
    return kind == null ? [] : [{ input, kind }];
  });
}

/** Just the kinds, for the sinks that filter by decoder rather than by channel. */
function useWiredKinds(inputs: readonly Input[]): string[] {
  return [...new Set(useWiredDecoders(inputs).map((wired) => wired.kind))];
}

function wireScope(inputs: readonly Input[]): WireScope {
  return {
    nodes: inputs.map((input) => input.node).join(","),
    sources: inputs.map((input) => `${input.deviceSet}:${input.channel.id}`).join(","),
  };
}

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
  const workspace = useWorkspaceContext();
  const audio = useChannelAudio(workspace.socket, input.deviceSet, input.channel.id);
  const label = workspace.graph.nodes.find((n) => n.id === input.node)?.label;
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
      <AudioHealth lostFrames={audio.lostFrames} underruns={audio.underruns} />
      {audio.error !== null && (
        <p role="alert" className="text-xs text-danger">
          {audio.error}
        </p>
      )}
    </div>
  );
}

/** Buffer diagnostics: useful when tuning the jitter buffer, noise for everyone else. */
function AudioHealth({
  lostFrames,
  underruns,
  show = import.meta.env.DEV,
}: {
  lostFrames: number;
  underruns: number;
  show?: boolean;
}) {
  if (!show || (lostFrames === 0 && underruns === 0)) return null;
  return (
    <span className="flex flex-wrap gap-1">
      {lostFrames > 0 && (
        <span
          className={CHIP}
          title="Audio that never reached the browser — dropped at the radio, the encoder or the link. Check the radio's overruns and the server, not this machine."
        >
          <span className="legend">Dropped</span>
          {(lostFrames / 48).toFixed(0)} ms
        </span>
      )}
      {underruns > 0 && (
        <span
          className={CHIP}
          title="Audio arrived but playback ran dry before it could be played — this machine's scheduling or a clock the buffer could not track. The buffer holds more after each one."
        >
          <span className="legend">Stalls</span>
          {underruns}
        </span>
      )}
    </span>
  );
}

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

/**
 * One readout per connected decoder: the picture a decoder is *holding* — the station it has
 * pieced together, the aircraft it is tracking, the text it has copied — rather than the frames
 * it received, which are a log and belong in one.
 *
 * Several decoders wired in stack their readouts, the way the map stacks layers. Only the ones
 * that hold something get a pane; a channel whose whole output is independent frames is named as
 * being read elsewhere instead of given an empty box.
 */
export function ReadoutFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const inputs = useInputs(node.id, "events");
  const readable = useWiredDecoders(inputs).filter((wired) => hasDecoderView(wired.kind));
  return (
    <NodeShell
      node={node}
      title="Readout"
      category="display"
      subtitle={inputs.length > 0 ? `${inputs.length} in` : undefined}
      live={readable.length > 0}
    >
      <FaceBody>
        {inputs.length === 0 ? (
          <FaceEmpty>Wire a decoder's events out to watch what it is holding.</FaceEmpty>
        ) : readable.length === 0 ? (
          <FaceEmpty>
            Nothing wired in holds a picture between frames — every one of these decodes to
            messages, which a decoder-log node is where you read.
          </FaceEmpty>
        ) : (
          readable.map(({ input, kind }) => (
            <div key={input.node} className="border-b border-line last:border-b-0">
              {readable.length > 1 && (
                <span className="legend block px-3 pt-2">
                  {workspace.graph.nodes.find((n) => n.id === input.node)?.label ??
                    input.channel.settings.params.type.toUpperCase()}
                </span>
              )}
              {/* Channel ids are allocated per device set, so two sets both have a channel 1;
                  scoping on the id alone would pour one set's output into this pane. */}
              <DecoderView
                kind={kind}
                scope={{ deviceSet: input.deviceSet, channel: input.channel.id }}
              />
            </div>
          ))
        )}
      </FaceBody>
    </NodeShell>
  );
}

export function VideoFace({ node }: { node: PatchNode }) {
  const inputs = useInputs(node.id, "video");
  return (
    <NodeShell
      node={node}
      title="Video"
      category="display"
      subtitle={inputs.length > 0 ? `${inputs.length} in` : undefined}
      live={inputs.length > 0}
    >
      <FaceBody>
        {inputs.length === 0 ? (
          <FaceEmpty>Wire a video channel's picture out to watch it.</FaceEmpty>
        ) : (
          inputs.map((input) => (
            <VideoView
              key={input.node}
              scope={{ deviceSet: input.deviceSet, channel: input.channel.id }}
            />
          ))
        )}
      </FaceBody>
    </NodeShell>
  );
}

/** The stored log, narrowed to the channels wired in — the wire is the filter, which is the whole
 * reason this is a node rather than a menu item. Two log nodes on different decoders are two
 * different logs, and clearing one clears only its own rows. */
export function DecoderLogFace({ node }: { node: PatchNode }) {
  const inputs = useInputs(node.id, "events");
  return (
    <NodeShell
      node={node}
      title="Decoder log"
      category="display"
      subtitle={inputs.length > 0 ? `${inputs.length} in` : undefined}
      live={inputs.length > 0}
    >
      <FaceBody scroll={false}>
        {inputs.length === 0 ? (
          <FaceEmpty>Wire decoders in; their frames are what this log holds.</FaceEmpty>
        ) : (
          <DecoderLogPanel wires={wireScope(inputs)} />
        )}
      </FaceBody>
    </NodeShell>
  );
}

export function CallRow({ call }: { call: VoiceCall }) {
  const destination =
    call.destination == null ? "Unknown" : `${call.group_call ? "TG" : "ID"} ${call.destination}`;
  const source = call.source == null ? "Unknown source" : `Radio ${call.source}`;
  const when = new Date(call.ended_at).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
  return (
    <article className="flex flex-col gap-2 border-b border-line p-2 last:border-b-0">
      <div className="flex min-w-0 flex-wrap items-center gap-1.5">
        <strong className="truncate font-mono text-xs text-ink">{destination}</strong>
        <span className={CHIP}>{source}</span>
        {call.slot != null && <span className={CHIP}>TS {call.slot}</span>}
        {call.color_code != null && <span className={CHIP}>CC {call.color_code}</span>}
        <span className="ml-auto font-mono text-[10px] text-ink-faint">
          {when} · {(call.duration_ms / 1000).toFixed(1)} s
        </span>
      </div>
      {call.encrypted ? (
        <span className="text-xs text-warning">Encrypted · metadata only</span>
      ) : call.audio != null ? (
        <audio
          className="h-8 w-full min-w-0"
          controls
          preload="none"
          src={callAudioUrl(call.audio.url)}
        />
      ) : (
        <span className="text-xs text-ink-dim">Audio was not retained.</span>
      )}
      {call.audio_error != null && (
        <p role="alert" className="text-xs text-danger">
          {call.audio_error}
        </p>
      )}
    </article>
  );
}

/** Fronts the decoder-log export API over the same wired channels the log node reads. */
export function ExportFace({ node }: { node: PatchNode }) {
  const inputs = useInputs(node.id, "events");
  const kinds = useWiredKinds(inputs);
  const wires = wireScope(inputs);
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
                <a key={format} className={BTN} href={decoderLogExportUrl(format, wires)} download>
                  {format.toUpperCase()}
                </a>
              ))}
            </div>
          </div>
        )}
      </FaceBody>
    </NodeShell>
  );
}

export function RecorderFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const set = deviceSetOf(workspace, node.id);
  // The lane this recorder's own wire names: wired to `iq3`, it must record stream 2, not the
  // radio's first.
  const stream = iqSourceOf(workspace.graph, node.id)?.stream ?? 0;
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
          <FaceEmpty>Wire a device's IQ out to record it.</FaceEmpty>
        ) : (
          <RecordControl set={set} stream={stream} />
        )}
      </FaceBody>
    </NodeShell>
  );
}

/** `deriveRecordControl` owns the two rules this face must not restate: a start needs a running
 * radio, and a faulted recording still reads as recording until it is explicitly stopped. */
function RecordControl({ set, stream }: { set: DeviceSet; stream: number }) {
  const record = useMutation({
    mutationFn: (action: RecordAction) => recordDeviceSet(set.id, action, stream),
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
          title={control.canStart ? "Record IQ to a SigMF pair" : "The radio must be running"}
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

export function ScannerFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const set = deviceSetOf(workspace, node.id);
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
          <FaceEmpty>
            Wire this out to a device; the scanner then drives that radio's tuning.
          </FaceEmpty>
        ) : (
          <ScannerPanel active={set} />
        )}
      </FaceBody>
    </NodeShell>
  );
}
