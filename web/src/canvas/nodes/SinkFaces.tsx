import { useMutation } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { Button } from "../../components/BaseControls";
import { BTN, BTN_DANGER, CHIP } from "../../components/controls";
import { DecoderLogPanel } from "../../components/DecoderLogPanel";
import { DecoderView, hasDecoderView } from "../../components/DecoderPanels";
import { DownloadMenu } from "../../components/DownloadMenu";
import {
  DEFAULT_LOG_FILTER,
  type EventGate,
  logDownloads,
  toQuery,
  type WireScope,
} from "../../components/decoderLog";
import { HuntPanel } from "../../components/HuntPanel";
import { DEFAULT_HUNT_SETTINGS } from "../../components/hunt";
import { MapPanel } from "../../components/MapPanel";
import { Readout, ReadoutRow } from "../../components/Readout";
import {
  deriveRecordControl,
  formatBytes,
  formatDuration,
  recordingElapsedS,
} from "../../components/recordings";
import { ScannerPanel } from "../../components/ScannerPanel";
import { Slider } from "../../components/Slider";
import { VideoView } from "../../components/VideoView";
import {
  callAudioUrl,
  recordChannelAudio,
  recordChannelBaseband,
  recordDeviceSet,
} from "../../lib/api";
import { useChannelAudio } from "../../lib/audio/useChannelAudio";
import { SAMPLE_RATE as AUDIO_RATE_HZ } from "../../lib/audio/worklet";
import { useDfStore } from "../../lib/df";
import {
  crossingSourcesOf,
  dfOverlay,
  dfSourcesOf,
  type RadarSource,
  radarSourcesOf,
} from "../../lib/dfOverlay";
import { type MapKind, mapKindsOf } from "../../lib/map/layers";
import { positionSourcesOf, usePositionStore } from "../../lib/position";
import { pushToast } from "../../lib/toasts";
import type {
  AudioRecordingStatus,
  DeviceSet,
  EventFilterNode,
  PatchNode,
  PatchNodeOf,
  RecordAction,
  RecordingStatus,
  VoiceCall,
} from "../../lib/types";
import {
  type EventPath,
  eventPathsOf,
  type Input,
  inputsOf,
  iqSourceOf,
  targetsOf,
} from "../binding";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { deviceSetOf } from "../workspaceDevice";
import { AudioSpectrogramView } from "./AudioSpectrogramView";
import { RADIO_IDLE, useFaceEmptyText } from "./faceCopy";
import { FaceBody, FaceEmpty, FaceFooter, NodeShell, useFaceActive } from "./NodeShell";

function useInputs(node: string, port: string): Input[] {
  const workspace = useWorkspaceContext();
  return inputsOf(
    workspace.graph,
    node,
    port,
    workspace.devices,
    workspace.channels,
    workspace.trunks,
  );
}

function useWiredDecoders(inputs: readonly Input[]): { input: Input; kind: string }[] {
  const workspace = useWorkspaceContext();
  return inputs.flatMap((input) => {
    const type = input.channel.settings.params.type;
    const kind = workspace.context.channelTypes.find((t) => t.type_id === type)?.decoder_kind;
    return kind == null ? [] : [{ input, kind }];
  });
}

function useWiredKinds(inputs: readonly Input[]): string[] {
  return [...new Set(useWiredDecoders(inputs).map((wired) => wired.kind))];
}

function wireScope(inputs: readonly Input[], paths: readonly EventPath[] = []): WireScope {
  return {
    nodes: inputs.map((input) => input.node).join(","),
    sources: inputs.map((input) => `${input.deviceSet}:${input.channel.id}`).join(","),
    gate: eventGate(inputs, paths),
  };
}

function eventGate(inputs: readonly Input[], paths: readonly EventPath[]): EventGate {
  const bySource: Record<string, EventFilterNode[][]> = {};
  for (const input of inputs) {
    const key = `${input.deviceSet}:${input.channel.id}`;
    const chains = paths.filter((path) => path.source === input.node).map((path) => path.filters);
    (bySource[key] ??= []).push(...chains);
  }
  const kinds = new Set<string>();
  for (const chains of Object.values(bySource)) {
    for (const chain of chains) {
      const named = chain.flatMap((filter) => filter.kinds ?? []);
      if (named.length === 0) {
        return { kinds: [], bySource };
      }
      for (const kind of named) {
        kinds.add(kind);
      }
    }
  }
  return { kinds: [...kinds].toSorted(), bySource };
}

export function SpeakerFace({ node }: { node: PatchNode }) {
  const inputs = useInputs(node.id, "audio");
  const empty = useFaceEmptyText(node.id, "audio", "Wire a channel's audio out to this speaker.");
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
          <FaceEmpty>{empty}</FaceEmpty>
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
        <Button
          type="button"
          className={audio.playing ? BTN_DANGER : BTN}
          onClick={() => {
            audio.resumeOutput();
            if (audio.playing) {
              audio.stop();
            } else {
              audio.start();
            }
          }}
        >
          {audio.playing ? "Stop" : audio.pending ? "…" : "Play"}
        </Button>
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
        <Button type="button" className={BTN} onClick={audio.resumeOutput}>
          Audio is suspended — click to resume
        </Button>
      )}
      <AudioSpectrogramView
        deviceSet={input.deviceSet}
        channel={input.channel.id}
        playing={audio.playing}
      />
      <AudioHealth lostFrames={audio.lostFrames} underruns={audio.underruns} />
      {audio.error !== null && (
        <p role="alert" className="text-xs text-danger">
          {audio.error}
        </p>
      )}
    </div>
  );
}

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
  const workspace = useWorkspaceContext();
  const inputs = useInputs(node.id, "events");
  const wired = useWiredKinds(inputs);
  const kinds = mapKindsOf(wired);
  const positions = positionSourcesOf(workspace.graph, node.id);
  const finders = dfSourcesOf(workspace.graph, node.id);
  const crossings = crossingSourcesOf(workspace.graph, node.id);
  const radars = radarSourcesOf(workspace.graph, node.id);
  const empty = useFaceEmptyText(
    node.id,
    "events",
    "Wire decoder events or a GPS position in to plot them.",
  );
  const anything =
    kinds.length > 0 ||
    positions.length > 0 ||
    finders.length > 0 ||
    crossings.length > 0 ||
    radars.length > 0;
  return (
    <NodeShell
      node={node}
      title="Map"
      category="display"
      subtitle={inputs.length > 0 ? `${inputs.length} in` : undefined}
      live={anything}
    >
      <FaceBody scroll={false}>
        {inputs.length === 0 && positions.length === 0 ? (
          <FaceEmpty>{empty}</FaceEmpty>
        ) : anything ? (
          <Plot
            kinds={kinds}
            positionNodes={positions}
            finders={finders}
            crossings={crossings}
            radars={radars}
          />
        ) : (
          <FaceEmpty>
            Nothing wired in reports a position. ADS-B, AIS and APRS do; the rest have nowhere to be
            drawn.
          </FaceEmpty>
        )}
      </FaceBody>
    </NodeShell>
  );
}

function Plot({
  kinds,
  positionNodes,
  finders,
  crossings,
  radars,
}: {
  kinds: readonly MapKind[];
  positionNodes: readonly string[];
  finders: readonly string[];
  crossings: readonly string[];
  radars: readonly RadarSource[];
}) {
  const byNode = useDfStore((store) => store.byNode);
  const here = usePositionStore((store) =>
    positionNodes.length === 0 ? undefined : store.sources[positionNodes[0] ?? ""]?.fix,
  );
  const df = dfOverlay(
    { finders, crossings, radars },
    byNode,
    Date.now(),
    here === undefined || here === null ? null : { lat: here.latitude, lon: here.longitude },
  );
  return (
    <MapPanel
      kinds={kinds}
      positionNodes={positionNodes}
      df={df}
      active={useFaceActive()}
      className="h-full min-h-0 w-full flex-1"
    />
  );
}

export function ReadoutFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const inputs = useInputs(node.id, "events");
  const readable = useWiredDecoders(inputs).filter((wired) => hasDecoderView(wired.kind));
  const empty = useFaceEmptyText(
    node.id,
    "events",
    "Wire a decoder's events output in. Decoders that build up a picture, like SSTV or VOR, show it here.",
  );
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
          <FaceEmpty>{empty}</FaceEmpty>
        ) : readable.length === 0 ? (
          <FaceEmpty>
            None of the wired decoders builds up a picture — they all decode to messages. Read those
            in a decoder-log node.
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
  const empty = useFaceEmptyText(
    node.id,
    "video",
    "Wire a video channel's picture out to watch it.",
  );
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
          <FaceEmpty>{empty}</FaceEmpty>
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

export function DecoderLogFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const inputs = useInputs(node.id, "events");
  const paths = eventPathsOf(workspace.graph, node.id);
  const empty = useFaceEmptyText(
    node.id,
    "events",
    "Wire decoders in; their frames are what this log holds.",
  );
  return (
    <NodeShell
      node={node}
      title="Decoder log"
      category="display"
      subtitle={inputs.length > 0 ? `${inputs.length} in` : undefined}
      live={inputs.length > 0}
    >
      {inputs.length === 0 ? (
        <FaceBody scroll={false}>
          <FaceEmpty>{empty}</FaceEmpty>
        </FaceBody>
      ) : (
        <DecoderLogPanel wires={wireScope(inputs, paths)} />
      )}
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

export function ExportFace({ node }: { node: PatchNode }) {
  const inputs = useInputs(node.id, "events");
  const kinds = useWiredKinds(inputs);
  const wires = wireScope(inputs);
  const empty = useFaceEmptyText(
    node.id,
    "events",
    "Wire decoders in; their stored rows are what gets exported.",
  );
  return (
    <NodeShell
      node={node}
      title="Export"
      category="sink"
      subtitle={kinds.length > 0 ? kinds.join(" · ") : undefined}
      live={inputs.length > 0}
    >
      <FaceBody>
        <FaceEmpty>
          {inputs.length === 0 ? empty : "Every row these decoders have logged, as one file."}
        </FaceEmpty>
      </FaceBody>
      <FaceFooter>
        <DownloadMenu
          choices={logDownloads(toQuery(DEFAULT_LOG_FILTER, wires))}
          disabled={inputs.length === 0}
        />
      </FaceFooter>
    </NodeShell>
  );
}

export function RecorderFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const set = deviceSetOf(workspace, node.id);
  const stream = iqSourceOf(workspace.graph, node.id)?.stream ?? 0;
  const empty = useFaceEmptyText(node.id, "iq", "Wire a device's IQ out to record it.");
  return (
    <NodeShell
      node={node}
      title="Recorder"
      category="sink"
      subtitle={set?.recording == null ? undefined : "recording"}
      live={set !== null}
    >
      {set === null ? (
        <FaceBody>
          <FaceEmpty>{empty}</FaceEmpty>
        </FaceBody>
      ) : (
        <RecordControl set={set} stream={stream} />
      )}
    </NodeShell>
  );
}

function RecordControl({ set, stream }: { set: DeviceSet; stream: number }) {
  const record = useMutation({
    mutationFn: (action: RecordAction) => recordDeviceSet(set.id, action, stream),
    onError: (error: Error) => pushToast(error.message),
  });
  const control = deriveRecordControl(set);
  const status = control.kind === "idle" ? null : control.status;
  const canStart = control.kind === "idle" && control.canStart;
  return (
    <>
      <FaceBody>
        {status === null ? (
          <FaceEmpty>
            {canStart
              ? "Ready. Recording writes a SigMF pair beside the server's other captures."
              : "The radio has to be running before it can be recorded."}
          </FaceEmpty>
        ) : (
          <>
            <RecordingReadout status={status} sampleRate={set.settings.sample_rate ?? 0} />
            {status.error != null && (
              <p role="alert" className="border-t border-line p-2 text-xs text-danger">
                {status.error}
              </p>
            )}
          </>
        )}
      </FaceBody>
      <FaceFooter>
        {status === null ? (
          <Button
            type="button"
            className={BTN}
            disabled={!canStart || record.isPending}
            title="Record IQ to a SigMF pair"
            onClick={() => record.mutate("start")}
          >
            <span aria-hidden className="text-danger">
              ●
            </span>
            Record
          </Button>
        ) : (
          <Button
            type="button"
            className={BTN_DANGER}
            disabled={record.isPending}
            onClick={() => record.mutate("stop")}
          >
            Stop
          </Button>
        )}
      </FaceFooter>
    </>
  );
}

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
    <Readout separated={false}>
      <ReadoutRow label="Elapsed">
        {formatDuration(recordingElapsedS(status, now, sampleRate))}
      </ReadoutRow>
      <ReadoutRow label="Written">{formatBytes(status.bytes)}</ReadoutRow>
      {status.overruns > 0 && <ReadoutRow label="Drops">{status.overruns}</ReadoutRow>}
      <ReadoutRow label="File">
        <span className="block truncate" title={status.file}>
          {status.file}
        </span>
      </ReadoutRow>
    </Readout>
  );
}

export function AudioRecorderFace({ node }: { node: PatchNode }) {
  const inputs = useInputs(node.id, "audio");
  const recording = inputs.filter((input) => input.channel.audio_recording != null).length;
  const empty = useFaceEmptyText(
    node.id,
    "audio",
    "Wire a channel's audio out to record what it sounds like.",
  );
  return (
    <NodeShell
      node={node}
      title="Audio recorder"
      category="sink"
      subtitle={
        recording === 0
          ? undefined
          : recording === 1
            ? "1 channel recording"
            : `${recording} channels recording`
      }
      live={inputs.length > 0}
    >
      <FaceBody>
        {inputs.length === 0 ? (
          <FaceEmpty>{empty}</FaceEmpty>
        ) : (
          inputs.map((input) => <AudioRecordInput key={input.node} input={input} />)
        )}
      </FaceBody>
    </NodeShell>
  );
}

function AudioRecordInput({ input }: { input: Input }) {
  const workspace = useWorkspaceContext();
  const label = workspace.graph.nodes.find((n) => n.id === input.node)?.label;
  const status = input.channel.audio_recording ?? null;
  const record = useMutation({
    mutationFn: (action: RecordAction) =>
      recordChannelAudio(input.deviceSet, input.channel.id, action),
    onError: (error: Error) => pushToast(error.message),
  });
  return (
    <div className="flex flex-col gap-1 border-b border-line p-2 last:border-b-0">
      <div className="flex items-center gap-2">
        <Button
          type="button"
          className={status === null ? BTN : BTN_DANGER}
          disabled={record.isPending}
          title={status === null ? "Record this channel's audio to a WAV file" : undefined}
          onClick={() => record.mutate(status === null ? "start" : "stop")}
        >
          {status === null ? (
            <>
              <span aria-hidden className="text-danger">
                ●
              </span>
              Record
            </>
          ) : (
            "Stop"
          )}
        </Button>
        <span className="legend truncate">
          {label ?? input.channel.settings.params.type.toUpperCase()}
        </span>
      </div>
      {status !== null && <AudioRecordingReadout status={status} />}
      {status?.error != null && (
        <p role="alert" className="text-xs text-danger">
          {status.error}
        </p>
      )}
    </div>
  );
}

function AudioRecordingReadout({ status }: { status: AudioRecordingStatus }) {
  return (
    <Readout separated={false}>
      <ReadoutRow label="Elapsed">{formatDuration(status.frames / AUDIO_RATE_HZ)}</ReadoutRow>
      <ReadoutRow label="Written">{formatBytes(status.bytes)}</ReadoutRow>
      <ReadoutRow label="File">
        <span className="block truncate" title={status.file}>
          {status.file}
        </span>
      </ReadoutRow>
    </Readout>
  );
}

export function BasebandRecorderFace({ node }: { node: PatchNode }) {
  const inputs = useInputs(node.id, "baseband");
  const recording = inputs.filter((input) => input.channel.baseband_recording != null).length;
  const empty = useFaceEmptyText(
    node.id,
    "baseband",
    "Wire a channel's baseband out to write its own IQ — down-converted, filtered and at the channel's rate — as a SigMF pair.",
  );
  return (
    <NodeShell
      node={node}
      title="Baseband recorder"
      category="sink"
      subtitle={
        recording === 0
          ? undefined
          : recording === 1
            ? "1 channel recording"
            : `${recording} channels recording`
      }
      live={inputs.length > 0}
    >
      <FaceBody>
        {inputs.length === 0 ? (
          <FaceEmpty>{empty}</FaceEmpty>
        ) : (
          inputs.map((input) => <BasebandRecordInput key={input.node} input={input} />)
        )}
      </FaceBody>
    </NodeShell>
  );
}

function BasebandRecordInput({ input }: { input: Input }) {
  const workspace = useWorkspaceContext();
  const label = workspace.graph.nodes.find((n) => n.id === input.node)?.label;
  const status = input.channel.baseband_recording ?? null;
  const record = useMutation({
    mutationFn: (action: RecordAction) =>
      recordChannelBaseband(input.deviceSet, input.channel.id, action),
    onError: (error: Error) => pushToast(error.message),
  });
  return (
    <div className="flex flex-col gap-1 border-b border-line p-2 last:border-b-0">
      <div className="flex items-center gap-2">
        <Button
          type="button"
          className={status === null ? BTN : BTN_DANGER}
          disabled={record.isPending}
          title={status === null ? "Record this channel's baseband to a SigMF pair" : undefined}
          onClick={() => record.mutate(status === null ? "start" : "stop")}
        >
          {status === null ? (
            <>
              <span aria-hidden className="text-danger">
                ●
              </span>
              Record
            </>
          ) : (
            "Stop"
          )}
        </Button>
        <span className="legend truncate">
          {label ?? input.channel.settings.params.type.toUpperCase()}
        </span>
      </div>
      {status !== null && <BasebandRecordingReadout status={status} />}
      {status?.error != null && (
        <p role="alert" className="text-xs text-danger">
          {status.error}
        </p>
      )}
    </div>
  );
}

function BasebandRecordingReadout({ status }: { status: RecordingStatus }) {
  return (
    <Readout separated={false}>
      <ReadoutRow label="Written">{formatBytes(status.bytes)}</ReadoutRow>
      <ReadoutRow label="Samples">{status.samples.toLocaleString()}</ReadoutRow>
      {status.overruns > 0 && <ReadoutRow label="Drops">{status.overruns}</ReadoutRow>}
      <ReadoutRow label="File">
        <span className="block truncate" title={status.file}>
          {status.file}
        </span>
      </ReadoutRow>
    </Readout>
  );
}

export function HuntFace({ node }: { node: PatchNode }) {
  if (node.kind !== "hunt") {
    return null;
  }
  return <HuntNodeFace node={node} />;
}

function HuntNodeFace({ node }: { node: PatchNodeOf<"hunt"> }) {
  const workspace = useWorkspaceContext();
  const set = deviceSetOf(workspace, node.id);
  const hunting = set?.hunt != null;
  const remember = (data: Partial<PatchNodeOf<"hunt">["data"]>): void => {
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, node.id, (current) =>
        current.kind === "hunt" ? { ...current, data: { ...current.data, ...data } } : current,
      ),
    }));
  };
  return (
    <NodeShell
      node={node}
      title="Signal hunt"
      category="feature"
      subtitle={hunting ? "owns this radio" : undefined}
      live={set !== null}
    >
      <HuntPanel
        active={set}
        settings={node.data.settings ?? DEFAULT_HUNT_SETTINGS}
        clicks={node.data.clicks ?? true}
        onSettings={(settings) => remember({ settings })}
        onClicks={(clicks) => remember({ clicks })}
        empty={
          targetsOf(workspace.graph, node.id, "control").length > 0
            ? RADIO_IDLE
            : "Wire this node's control out to a device; the hunt then parks that radio on one frequency."
        }
      />
    </NodeShell>
  );
}

export function ScannerFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const set = deviceSetOf(workspace, node.id);
  const scanning = set?.scanner != null;
  return (
    <NodeShell
      node={node}
      title="Scanner"
      category="feature"
      subtitle={scanning ? "owns this radio" : undefined}
      live={set !== null}
    >
      <ScannerPanel
        active={set}
        others={workspace.deviceSets}
        session={workspace.scanSession}
        empty={
          targetsOf(workspace.graph, node.id, "control").length > 0
            ? RADIO_IDLE
            : "Wire this node's control out to a device; the scanner then drives that radio's tuning."
        }
      />
    </NodeShell>
  );
}
