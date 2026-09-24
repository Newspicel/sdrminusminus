import { Circle } from "lucide-react";
import { useEffect, useState } from "react";
import { Button } from "../../components/BaseControls";
import { BTN, BTN_DANGER, CHIP } from "../../components/controls";
import { DecoderLogPanel } from "../../components/DecoderLogPanel";
import { DecoderView, hasDecoderView } from "../../components/DecoderPanels";
import { DevOnly } from "../../components/DevOnly";
import { DownloadMenu } from "../../components/DownloadMenu";
import {
  DEFAULT_LOG_FILTER,
  logDownloads,
  toQuery,
  type WireScope,
} from "../../components/decoderLog";
import { formatBytes } from "../../components/format";
import { HuntPanel } from "../../components/HuntPanel";
import { Icon } from "../../components/Icon";
import { MapPanel } from "../../components/MapPanel";
import { Readout, ReadoutRow } from "../../components/Readout";
import { formatDuration, recordingElapsedS } from "../../components/recordings";
import { ScannerPanel } from "../../components/ScannerPanel";
import { Slider } from "../../components/Slider";
import { VideoView } from "../../components/VideoView";
import { callAudioUrl } from "../../lib/api";
import { monitorKey } from "../../lib/audio/monitor";
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
import type {
  AudioRecordingStatus,
  PatchNode,
  PatchNodeOf,
  RecordingStatus,
  VoiceCall,
} from "../../lib/types";
import { useNow } from "../../lib/useNow";
import { eventSourcesOf, type Input, inputsOf, wiredSourcesOf } from "../binding";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { decoderOf, deviceSetOf } from "../workspaceDevice";
import { AudioSpectrogramView } from "./AudioSpectrogramView";
import { recordingFor } from "./audioRecorder";
import { kindsOffered } from "./eventFilter";
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
    workspace.owners,
  );
}

function inputKey(input: Input): string {
  return monitorKey(input.deviceSet, input.channel.id, input.fx);
}

function useWiredDecoders(inputs: readonly Input[]): { input: Input; kind: string }[] {
  const workspace = useWorkspaceContext();
  return inputs.flatMap((input) => {
    const type = input.channel.settings.params.type;
    const kind = workspace.context.channelTypes.find((t) => t.type_id === type)?.decoder_kind;
    return kind == null ? [] : [{ input, kind }];
  });
}

function useWiredKinds(sink: string): string[] {
  const workspace = useWorkspaceContext();
  return kindsOffered(wiredSourcesOf(workspace.graph, sink), workspace.context.channelTypes);
}

function useWireScope(sink: string): WireScope {
  const workspace = useWorkspaceContext();
  return { sink, wired: eventSourcesOf(workspace.graph, sink).length > 0 };
}

export function SpeakerFace({ node }: { node: PatchNode }) {
  const inputs = useInputs(node.id, "audio");

  return (
    <NodeShell node={node} title="Speaker" category="output">
      <FaceBody>
        {inputs.length === 0 ? (
          <FaceEmpty hint="Wire a channel's audio in" />
        ) : (
          inputs.map((input) => <AudioInput key={inputKey(input)} input={input} />)
        )}
      </FaceBody>
    </NodeShell>
  );
}

function AudioInput({ input }: { input: Input }) {
  const workspace = useWorkspaceContext();
  const audio = useChannelAudio(workspace.socket, input.deviceSet, input.channel.id, input.fx);
  const active = audio.playing || audio.pending || audio.suspended;
  const label = workspace.graph.nodes.find((n) => n.id === input.node)?.label;
  return (
    <div className="flex flex-col gap-1 border-b border-line p-2 last:border-b-0">
      <div className="flex items-center gap-2">
        <Button
          type="button"
          className={active ? BTN_DANGER : BTN}
          onClick={() => {
            audio.resumeOutput();
            if (active) {
              audio.stop();
            } else {
              audio.start();
            }
          }}
        >
          {active ? "Stop" : "Play"}
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
        <Button
          type="button"
          className={BTN}
          onClick={audio.resumeOutput}
          title="Audio output is suspended"
        >
          Resume audio
        </Button>
      )}
      <AudioSpectrogramView
        source={monitorKey(input.deviceSet, input.channel.id, input.fx)}
        playing={audio.playing}
      />
      <DevOnly>
        <AudioHealth
          lostFrames={audio.lostFrames}
          underruns={audio.underruns}
          bufferedMs={audio.bufferedMs}
          trimmedMs={audio.trimmedMs}
        />
      </DevOnly>
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
  bufferedMs = 0,
  trimmedMs = 0,
}: {
  lostFrames: number;
  underruns: number;
  bufferedMs?: number;
  trimmedMs?: number;
}) {
  if (lostFrames === 0 && underruns === 0 && bufferedMs === 0 && trimmedMs === 0) return null;
  return (
    <span className="flex flex-wrap gap-1">
      {bufferedMs > 0 && (
        <span className={CHIP} title="Audio waiting for playback">
          <span className="legend">Buffer</span>
          {bufferedMs.toFixed(0)} ms
        </span>
      )}
      {trimmedMs > 0 && (
        <span className={CHIP} title="Old audio discarded to stay live">
          <span className="legend">Trimmed</span>
          {trimmedMs.toFixed(0)} ms
        </span>
      )}
      {lostFrames > 0 && (
        <span
          className={CHIP}
          title="Audio lost before playback: dropped at the radio, the encoder or the link, or decoded too late on this machine to be played."
        >
          <span className="legend">Dropped</span>
          {(lostFrames / 48).toFixed(0)} ms
        </span>
      )}
      {underruns > 0 && (
        <span
          className={CHIP}
          title="Audio arrived but playback ran dry before it could be played: this machine's scheduling or a clock the buffer could not track. The buffer holds more after each one."
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
  const wired = useWiredKinds(node.id);
  const kinds = mapKindsOf(wired);
  const positions = positionSourcesOf(workspace.graph, node.id);
  const finders = dfSourcesOf(workspace.graph, node.id);
  const crossings = crossingSourcesOf(workspace.graph, node.id);
  const radars = radarSourcesOf(workspace.graph, node.id);
  return (
    <NodeShell node={node} title="Map" category="output">
      <FaceBody scroll={false}>
        <Plot
          kinds={kinds}
          positionNodes={positions}
          finders={finders}
          crossings={crossings}
          radars={radars}
        />
      </FaceBody>
    </NodeShell>
  );
}

const OVERLAY_TICK_MS = 1_000;

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
  const now = useNow(OVERLAY_TICK_MS);
  const here = usePositionStore((store) =>
    positionNodes.length === 0 ? undefined : store.sources[positionNodes[0] ?? ""]?.fix,
  );
  const df = dfOverlay(
    { finders, crossings, radars },
    byNode,
    now,
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
  const wires = useWireScope(node.id);
  const monitor = eventSourcesOf(workspace.graph, node.id).some((source) =>
    workspace.graph.nodes.some(
      (candidate) => candidate.id === source && candidate.kind === "spectrum_monitor",
    ),
  );
  if (monitor) {
    return (
      <NodeShell node={node} title="Readout" category="output">
        <DecoderLogPanel wires={wires} />
      </NodeShell>
    );
  }
  return (
    <NodeShell node={node} title="Readout" category="output">
      <FaceBody>
        {inputs.length === 0 ? (
          <FaceEmpty hint="Wire a decoder's events in" />
        ) : readable.length === 0 ? (
          <FaceEmpty hint="No wired decoder builds up a picture" />
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
  return (
    <NodeShell node={node} title="Video" category="output">
      <FaceBody>
        {inputs.length === 0 ? (
          <FaceEmpty hint="Wire a video channel's picture in" />
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
  const wires = useWireScope(node.id);
  return (
    <NodeShell node={node} title="Decoder log" category="output">
      <DecoderLogPanel wires={wires} />
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
  const wires = useWireScope(node.id);
  return (
    <NodeShell node={node} title="Export" category="output">
      <FaceBody>
        <FaceEmpty hint={!wires.wired ? "Wire decoders in" : "Every logged row, as one file"} />
      </FaceBody>
      <FaceFooter>
        <DownloadMenu
          choices={logDownloads(toQuery(DEFAULT_LOG_FILTER, wires))}
          disabled={!wires.wired}
        />
      </FaceFooter>
    </NodeShell>
  );
}

type RecorderNodeOf = PatchNodeOf<"recorder" | "audio_recorder" | "baseband_recorder">;

function isRecorder(node: PatchNode): node is RecorderNodeOf {
  return (
    node.kind === "recorder" || node.kind === "audio_recorder" || node.kind === "baseband_recorder"
  );
}

function RecorderSwitch({ node, title }: { node: RecorderNodeOf; title: string }) {
  const workspace = useWorkspaceContext();
  const recording = node.data?.recording ?? false;
  const switchTo = (next: boolean) => {
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, node.id, (current) =>
        isRecorder(current) ? { ...current, data: { ...current.data, recording: next } } : current,
      ),
    }));
  };
  return (
    <Button
      type="button"
      className={recording ? BTN_DANGER : BTN}
      title={recording ? undefined : title}
      onClick={() => switchTo(!recording)}
    >
      {recording ? (
        "Stop"
      ) : (
        <>
          <span className="flex text-danger">
            <Icon glyph={Circle} size={12} filled />
          </span>
          Record
        </>
      )}
    </Button>
  );
}

export function RecorderFace({ node }: { node: PatchNode }) {
  if (node.kind !== "recorder") {
    return null;
  }
  return <IqRecorder node={node} />;
}

function IqRecorder({ node }: { node: PatchNodeOf<"recorder"> }) {
  const workspace = useWorkspaceContext();
  const set = deviceSetOf(workspace, node.id);
  const status = set?.recording ?? null;
  const waiting = (node.data?.recording ?? false) && status === null;
  return (
    <NodeShell node={node} title="Recorder" category="output">
      <FaceBody>
        {status === null ? (
          <FaceEmpty
            hint={set === null ? "Wire a device's IQ in" : waiting ? "Waiting" : undefined}
          />
        ) : (
          <>
            <RecordingReadout status={status} sampleRate={set?.settings.sample_rate ?? 0} />
            {status.error != null && (
              <p role="alert" className="border-t border-line p-2 text-xs text-danger">
                {status.error}
              </p>
            )}
          </>
        )}
      </FaceBody>
      {set !== null && (
        <FaceFooter>
          <RecorderSwitch node={node} title="Record IQ to a SigMF pair" />
        </FaceFooter>
      )}
    </NodeShell>
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
      <DevOnly>
        {status.overruns > 0 && <ReadoutRow label="Drops">{status.overruns}</ReadoutRow>}
      </DevOnly>
      <ReadoutRow label="File">
        <span className="block truncate" title={status.file}>
          {status.file}
        </span>
      </ReadoutRow>
    </Readout>
  );
}

export function AudioRecorderFace({ node }: { node: PatchNode }) {
  if (node.kind !== "audio_recorder") {
    return null;
  }
  return <AudioRecorder node={node} />;
}

function AudioRecorder({ node }: { node: PatchNodeOf<"audio_recorder"> }) {
  const inputs = useInputs(node.id, "audio");
  const recording = node.data?.recording ?? false;
  return (
    <NodeShell node={node} title="Audio recorder" category="output">
      <FaceBody>
        {inputs.length === 0 ? (
          <FaceEmpty hint="Wire a channel's audio in" />
        ) : (
          <>
            <div className="border-b border-line p-2">
              <RecorderSwitch node={node} title="Record every wired input to its own WAV file" />
            </div>
            {inputs.map((input) => (
              <AudioRecordInput key={inputKey(input)} input={input} recording={recording} />
            ))}
          </>
        )}
      </FaceBody>
    </NodeShell>
  );
}

function AudioRecordInput({ input, recording }: { input: Input; recording: boolean }) {
  const workspace = useWorkspaceContext();
  const label = workspace.graph.nodes.find((n) => n.id === input.node)?.label;
  const status = recordingFor(input.channel, input.fx);
  return (
    <div className="flex flex-col gap-1 border-b border-line p-2 last:border-b-0">
      <div className="flex items-center gap-2">
        <span className="legend truncate">
          {label ?? input.channel.settings.params.type.toUpperCase()}
        </span>
        {recording && status === null && <span className="legend">Waiting</span>}
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
  if (node.kind !== "baseband_recorder") {
    return null;
  }
  return <BasebandRecorder node={node} />;
}

function BasebandRecorder({ node }: { node: PatchNodeOf<"baseband_recorder"> }) {
  const inputs = useInputs(node.id, "baseband");
  const recording = node.data?.recording ?? false;
  return (
    <NodeShell node={node} title="Baseband recorder" category="output">
      <FaceBody>
        {inputs.length === 0 ? (
          <FaceEmpty hint="Wire a channel's baseband in" />
        ) : (
          <>
            <div className="border-b border-line p-2">
              <RecorderSwitch
                node={node}
                title="Record every wired channel's baseband to its own SigMF pair"
              />
            </div>
            {inputs.map((input) => (
              <BasebandRecordInput key={input.node} input={input} recording={recording} />
            ))}
          </>
        )}
      </FaceBody>
    </NodeShell>
  );
}

function BasebandRecordInput({ input, recording }: { input: Input; recording: boolean }) {
  const workspace = useWorkspaceContext();
  const label = workspace.graph.nodes.find((n) => n.id === input.node)?.label;
  const status = input.channel.baseband_recording ?? null;
  return (
    <div className="flex flex-col gap-1 border-b border-line p-2 last:border-b-0">
      <div className="flex items-center gap-2">
        <span className="legend truncate">
          {label ?? input.channel.settings.params.type.toUpperCase()}
        </span>
        {recording && status === null && <span className="legend">Waiting</span>}
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
      <DevOnly>
        {status.overruns > 0 && <ReadoutRow label="Drops">{status.overruns}</ReadoutRow>}
      </DevOnly>
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
  const decoder = decoderOf(workspace, node.id);
  const remember = (data: Partial<PatchNodeOf<"hunt">["data"]>): void => {
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, node.id, (current) =>
        current.kind === "hunt" ? { ...current, data: { ...current.data, ...data } } : current,
      ),
    }));
  };
  return (
    <NodeShell node={node} title="Signal hunt" category="tool">
      <HuntPanel
        target={decoder}
        clicks={node.data.clicks ?? true}
        onClicks={(clicks) => remember({ clicks })}
        hint="Wire this node's control out to a decoder"
      />
    </NodeShell>
  );
}

export function ScannerFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const decoder = decoderOf(workspace, node.id);
  const set = decoder?.set ?? null;
  return (
    <NodeShell node={node} title="Scanner" category="tool">
      <ScannerPanel
        active={set}
        channel={decoder?.channel ?? null}
        hint="Wire this node's control out to a decoder"
      />
    </NodeShell>
  );
}
