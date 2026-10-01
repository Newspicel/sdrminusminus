import { Circle, Play, Square } from "lucide-react";
import { useState } from "react";
import { Button } from "../../components/BaseControls";
import { BTN, BTN_DANGER, ICON_BTN } from "../../components/controls";
import { DecoderLogPanel } from "../../components/DecoderLogPanel";
import { DecoderView, hasDecoderView } from "../../components/DecoderPanels";
import { DownloadMenu } from "../../components/DownloadMenu";
import { meterTone } from "../../components/dbfs";
import {
  DEFAULT_LOG_FILTER,
  logDownloads,
  toQuery,
  type WireScope,
} from "../../components/decoderLog";
import { FaceFault } from "../../components/face/Fault";
import { GainMeter, MeterRow } from "../../components/face/Meter";
import { Readout, Readouts } from "../../components/face/Readouts";
import { FaceStats, Stat } from "../../components/face/Stats";
import { DROPS_HINT, formatBytes, formatCount } from "../../components/format";
import { HuntPanel } from "../../components/HuntPanel";
import { Icon } from "../../components/Icon";
import { formatDuration, recordingElapsedS } from "../../components/recordings";
import { ScannerPanel } from "../../components/ScannerPanel";
import { Tip } from "../../components/Tip";
import { type VideoSignal, VideoView, videoSignalText } from "../../components/VideoView";
import { monitorKey } from "../../lib/audio/monitor";
import { resumeAudioOutput } from "../../lib/audio/sink";
import { useChannelAudio } from "../../lib/audio/useChannelAudio";
import { SAMPLE_RATE as AUDIO_RATE_HZ } from "../../lib/audio/worklet";
import { overlaySourcesOf } from "../../lib/dfOverlay";
import { mapKindsOf } from "../../lib/map/layers";
import { positionSourcesOf } from "../../lib/position";
import type { PatchNode, PatchNodeOf, RecordingStatus } from "../../lib/types";
import { useNow } from "../../lib/useNow";
import { eventSourcesOf, hasWire, type Input, inputsOf, wiredSourcesOf } from "../binding";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { huntSweepOf } from "../newNode";
import { decoderOf, deviceSetOf } from "../workspaceDevice";
import { AudioSpectrogramView } from "./AudioSpectrogramView";
import { recordingFor } from "./audioRecorder";
import { kindsOffered } from "./eventFilter";
import { MapPlot } from "./MapPlot";
import { FaceBody, FaceEmpty, FaceFooter, NodeShell } from "./NodeShell";
import {
  type SpeakerHealth,
  speakerHealthShown,
  useAudioPeakDb,
  useSpeakerHealth,
} from "./speaker";

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
  const health = useSpeakerHealth(inputs, import.meta.env.DEV);
  return (
    <NodeShell node={node} title="Speaker" category="output">
      <FaceBody>
        {inputs.length === 0 ? (
          <FaceEmpty hint="Wire a channel's audio in" />
        ) : (
          inputs.map((input, index) => (
            <AudioInput
              key={inputKey(input)}
              input={input}
              port={index === 0 ? "audio" : undefined}
            />
          ))
        )}
      </FaceBody>
      {inputs.length > 0 && speakerHealthShown(health) && (
        <FaceFooter>
          <AudioHealth health={health} />
          {health.suspended && (
            <Button
              type="button"
              className={BTN}
              onClick={resumeAudioOutput}
              title="Audio output is suspended"
            >
              Resume audio
            </Button>
          )}
        </FaceFooter>
      )}
    </NodeShell>
  );
}

function AudioInput({ input, port }: { input: Input; port?: string }) {
  const workspace = useWorkspaceContext();
  const audio = useChannelAudio(workspace.socket, input.deviceSet, input.channel.id, input.fx);
  const active = audio.playing || audio.pending || audio.suspended;
  const source = monitorKey(input.deviceSet, input.channel.id, input.fx);
  const peakDb = useAudioPeakDb(source, audio.playing);
  const name =
    workspace.graph.nodes.find((n) => n.id === input.node)?.label ??
    input.channel.settings.params.type.toUpperCase();
  return (
    <div className="flex flex-col gap-1 border-b border-line p-2 last:border-b-0">
      <MeterRow
        label={
          <span className="legend truncate" title={name}>
            {name}
          </span>
        }
        port={port}
        meter={
          <GainMeter
            label={`${name} volume`}
            className="min-w-0 flex-1"
            min={0}
            max={1}
            step={0.01}
            value={audio.volume}
            peakDb={peakDb ?? null}
            tone={meterTone(peakDb, false)}
            onChange={audio.setVolume}
          />
        }
        readout={`${Math.round(audio.volume * 100)}%`}
        trailing={
          <PlayToggle
            active={active}
            onToggle={() => {
              audio.resumeOutput();
              if (active) {
                audio.stop();
              } else {
                audio.start();
              }
            }}
          />
        }
      />
      <AudioSpectrogramView source={source} playing={audio.playing} />
      {audio.error !== null && <FaceFault message={audio.error} />}
    </div>
  );
}

function PlayToggle({ active, onToggle }: { active: boolean; onToggle: () => void }) {
  return (
    <Tip
      text={active ? "Stop listening" : "Listen"}
      render={
        <Button
          type="button"
          className={`${ICON_BTN} ${active ? "bg-accent/15 text-accent" : ""}`}
          aria-label={active ? "Stop" : "Play"}
          aria-pressed={active}
          onClick={onToggle}
        />
      }
    >
      <Icon glyph={active ? Square : Play} size={14} filled={active} />
    </Tip>
  );
}

function AudioHealth({ health }: { health: SpeakerHealth }) {
  return (
    <FaceStats>
      {health.bufferedMs > 0 && (
        <Stat label="Buffer" title="Audio waiting for playback">
          {health.bufferedMs.toFixed(0)} ms
        </Stat>
      )}
      {health.trimmedMs > 0 && (
        <Stat label="Trimmed" title="Old audio discarded to stay live">
          {health.trimmedMs.toFixed(0)} ms
        </Stat>
      )}
      {health.droppedMs > 0 && (
        <Stat
          label="Dropped"
          tone="warn"
          title="Audio lost before playback: dropped at the radio, the encoder or the link, or decoded too late on this machine to be played."
        >
          {health.droppedMs.toFixed(0)} ms
        </Stat>
      )}
      {health.underruns > 0 && (
        <Stat
          label="Stalls"
          tone="warn"
          title="Audio arrived but playback ran dry before it could be played: this machine's scheduling or a clock the buffer could not track. The buffer holds more after each one."
        >
          {health.underruns}
        </Stat>
      )}
    </FaceStats>
  );
}

export function MapFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const kinds = mapKindsOf(useWiredKinds(node.id));
  return (
    <NodeShell node={node} title="Map" category="output">
      <FaceBody scroll={false}>
        <MapPlot
          kinds={kinds}
          positionNodes={positionSourcesOf(workspace.graph, node.id)}
          sources={overlaySourcesOf(workspace.graph, node.id)}
        />
      </FaceBody>
    </NodeShell>
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
  const nameOf = useInputName();
  const [signals, setSignals] = useState<Readonly<Record<string, VideoSignal | null>>>({});
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
              onSignal={(signal) => setSignals((held) => ({ ...held, [input.node]: signal }))}
            />
          ))
        )}
      </FaceBody>
      {inputs.length > 0 && (
        <FaceFooter>
          <FaceStats>
            {inputs.map((input) => {
              const signal = signals[input.node] ?? null;
              return (
                <Stat
                  key={input.node}
                  label={inputs.length > 1 ? nameOf(input) : "Picture"}
                  title="Decoded picture size and sync"
                  tone={signal?.live === true ? undefined : "warn"}
                >
                  {videoSignalText(signal)}
                </Stat>
              );
            })}
          </FaceStats>
        </FaceFooter>
      )}
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

function FileReadout({ label, file }: { label: string; file: string }) {
  return (
    <Readout label={label} title={file}>
      {file}
    </Readout>
  );
}

function RecordStats({
  elapsedS,
  bytes,
  samples,
  overruns,
}: {
  elapsedS?: number;
  bytes: number;
  samples?: number;
  overruns: number;
}) {
  return (
    <FaceStats>
      {elapsedS !== undefined && (
        <Stat label="Time" title="Recording length">
          {formatDuration(elapsedS)}
        </Stat>
      )}
      <Stat label="Written" title="Bytes on disk">
        {formatBytes(bytes)}
      </Stat>
      {samples !== undefined && (
        <Stat label="Samples" title="Samples written">
          {formatCount(samples)}
        </Stat>
      )}
      {overruns > 0 && (
        <Stat label="Drops" title={DROPS_HINT} tone="warn">
          {formatCount(overruns)}
        </Stat>
      )}
    </FaceStats>
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
            <Readouts ruled={false}>
              <FileReadout label="File" file={status.file} />
            </Readouts>
            {status.error != null && <FaceFault message={status.error} />}
          </>
        )}
      </FaceBody>
      {set !== null && (
        <FaceFooter>
          {status !== null && (
            <IqRecordStats status={status} sampleRate={set.settings.sample_rate ?? 0} />
          )}
          <RecorderSwitch node={node} title="Record IQ to a SigMF pair" />
        </FaceFooter>
      )}
    </NodeShell>
  );
}

function IqRecordStats({ status, sampleRate }: { status: RecordingStatus; sampleRate: number }) {
  const now = useNow(1000);
  return (
    <RecordStats
      elapsedS={recordingElapsedS(status, now, sampleRate)}
      bytes={status.bytes}
      overruns={status.overruns}
    />
  );
}

export function AudioRecorderFace({ node }: { node: PatchNode }) {
  if (node.kind !== "audio_recorder") {
    return null;
  }
  return <AudioRecorder node={node} />;
}

function useInputName(): (input: Input) => string {
  const workspace = useWorkspaceContext();
  return (input) =>
    workspace.graph.nodes.find((n) => n.id === input.node)?.label ??
    input.channel.settings.params.type.toUpperCase();
}

function AudioRecorder({ node }: { node: PatchNodeOf<"audio_recorder"> }) {
  const inputs = useInputs(node.id, "audio");
  const nameOf = useInputName();
  const recording = node.data?.recording ?? false;
  const takes = inputs.map((input) => ({ input, status: recordingFor(input.channel, input.fx) }));
  const held = takes.flatMap(({ status }) => (status === null ? [] : [status]));
  return (
    <NodeShell node={node} title="Audio recorder" category="output">
      <FaceBody>
        {inputs.length === 0 ? (
          <FaceEmpty hint="Wire a channel's audio in" />
        ) : (
          <>
            <Readouts ruled={false}>
              {takes.map(({ input, status }) =>
                status === null ? (
                  <Readout key={inputKey(input)} label={nameOf(input)}>
                    {recording ? "Waiting" : "-"}
                  </Readout>
                ) : (
                  <FileReadout key={inputKey(input)} label={nameOf(input)} file={status.file} />
                ),
              )}
            </Readouts>
            {takes.map(({ input, status }) =>
              status?.error == null ? null : (
                <FaceFault key={inputKey(input)} message={`${nameOf(input)}: ${status.error}`} />
              ),
            )}
          </>
        )}
      </FaceBody>
      {inputs.length > 0 && (
        <FaceFooter>
          {held.length > 0 && (
            <RecordStats
              elapsedS={Math.max(...held.map((status) => status.frames)) / AUDIO_RATE_HZ}
              bytes={sum(held.map((status) => status.bytes))}
              overruns={0}
            />
          )}
          <RecorderSwitch node={node} title="Record every wired input to its own WAV file" />
        </FaceFooter>
      )}
    </NodeShell>
  );
}

function sum(values: readonly number[]): number {
  return values.reduce((total, value) => total + value, 0);
}

export function BasebandRecorderFace({ node }: { node: PatchNode }) {
  if (node.kind !== "baseband_recorder") {
    return null;
  }
  return <BasebandRecorder node={node} />;
}

function BasebandRecorder({ node }: { node: PatchNodeOf<"baseband_recorder"> }) {
  const inputs = useInputs(node.id, "baseband");
  const nameOf = useInputName();
  const recording = node.data?.recording ?? false;
  const takes = inputs.map((input) => ({
    input,
    status: input.channel.baseband_recording ?? null,
  }));
  const held = takes.flatMap(({ status }) => (status === null ? [] : [status]));
  return (
    <NodeShell node={node} title="Baseband recorder" category="output">
      <FaceBody>
        {inputs.length === 0 ? (
          <FaceEmpty hint="Wire a channel's baseband in" />
        ) : (
          <>
            <Readouts ruled={false}>
              {takes.map(({ input, status }) =>
                status === null ? (
                  <Readout key={input.node} label={nameOf(input)}>
                    {recording ? "Waiting" : "-"}
                  </Readout>
                ) : (
                  <FileReadout key={input.node} label={nameOf(input)} file={status.file} />
                ),
              )}
            </Readouts>
            {takes.map(({ input, status }) =>
              status?.error == null ? null : (
                <FaceFault key={input.node} message={`${nameOf(input)}: ${status.error}`} />
              ),
            )}
          </>
        )}
      </FaceBody>
      {inputs.length > 0 && (
        <FaceFooter>
          {held.length > 0 && (
            <RecordStats
              bytes={sum(held.map((status) => status.bytes))}
              samples={sum(held.map((status) => status.samples))}
              overruns={sum(held.map((status) => status.overruns))}
            />
          )}
          <RecorderSwitch
            node={node}
            title="Record every wired channel's baseband to its own SigMF pair"
          />
        </FaceFooter>
      )}
    </NodeShell>
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
  const sweep = huntSweepOf(node, workspace.context.catalog);
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
        node={node.id}
        target={decoder}
        clicks={node.data.clicks ?? true}
        onClicks={(clicks) => remember({ clicks })}
        hint="Wire this node's control out to a decoder"
        positionWired={hasWire(workspace.graph, node.id, "position")}
        sweep={sweep}
        onSweep={(next) => {
          if (sweep !== null) {
            remember({ sweep: { ...sweep, ...next } });
          }
        }}
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
