import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Button, Input } from "../../components/BaseControls";
import { BTN, BTN_DANGER, BTN_PRIMARY, FIELD, LABEL } from "../../components/controls";
import { formatHz, formatSignedKhz } from "../../components/format";
import { MapPanel } from "../../components/MapPanel";
import { OffsetStepper } from "../../components/OffsetStepper";
import type { SpectrumFrame } from "../../lib/frame";
import { type PositionSample, positionSourcesOf, usePositionStore } from "../../lib/position";
import {
  measureSignalDbfs,
  signalOffsetLimitHz,
  signalSurveyCsv,
  useSignalSurveyStore,
} from "../../lib/signalSurvey";
import { spectrumHub } from "../../lib/spectrum";
import type { PatchNode, PatchNodeOf } from "../../lib/types";
import { iqSourceOf } from "../binding";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { deviceSetOf } from "../workspaceDevice";
import { RADIO_IDLE } from "./faceCopy";
import { FaceBody, FaceEmpty, NodeShell, useFaceActive } from "./NodeShell";

const LEVEL_REFRESH_MS = 200;
const MAX_OFFSET_HZ = 1_000_000_000_000;

export function SignalMapFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const set = deviceSetOf(workspace, node.id);
  const iq = iqSourceOf(workspace.graph, node.id);
  const positionNode = positionSourcesOf(workspace.graph, node.id)[0];
  const position = usePositionStore((store) =>
    positionNode === undefined ? undefined : store.sources[positionNode]?.history.at(-1),
  );

  if (node.kind !== "signal_map") {
    return null;
  }

  return (
    <NodeShell
      node={node}
      title="Signal survey"
      category="display"
      subtitle={set === null ? undefined : formatSignedKhz(node.data.offset_hz)}
      live={set !== null && position !== undefined}
    >
      <FaceBody scroll={false}>
        {set === null || iq === null || positionNode === undefined ? (
          <FaceEmpty>
            {iq !== null && set === null
              ? RADIO_IDLE
              : set === null || iq === null
                ? "Wire a device's IQ and a GPS position in to survey signal strength."
                : "Wire a GPS position in to place signal readings on the map."}
          </FaceEmpty>
        ) : (
          <SignalSurvey
            node={node}
            deviceSet={set.id}
            stream={iq.stream}
            positionNode={positionNode}
            position={position}
          />
        )}
      </FaceBody>
    </NodeShell>
  );
}

function SignalSurvey({
  node,
  deviceSet,
  stream,
  positionNode,
  position,
}: {
  node: PatchNodeOf<"signal_map">;
  deviceSet: number;
  stream: number;
  positionNode: string;
  position: PositionSample | undefined;
}) {
  const workspace = useWorkspaceContext();
  const active = useFaceActive();
  const session = useSignalSurveyStore((store) => store.sessions[node.id]);
  const samples = session?.samples ?? [];
  const recording = session?.recording ?? false;
  const setRecording = useSignalSurveyStore((store) => store.setRecording);
  const observe = useSignalSurveyStore((store) => store.observe);
  const clear = useSignalSurveyStore((store) => store.clear);

  const [live, setLive] = useState<{
    level: number | null;
    targetHz: number;
    centerHz: number;
    spanHz: number;
  } | null>(null);
  const [clearArmed, setClearArmed] = useState(false);
  const positionRef = useRef(position);
  const recordingRef = useRef(recording);
  const surveyFrequencyRef = useRef(samples[0]?.frequencyHz);
  const offsetRef = useRef(node.data.offset_hz);
  const bandwidthRef = useRef(node.data.bandwidth_hz);
  const lastRecordedRef = useRef(0);
  const lastLevelRenderRef = useRef(0);
  useLayoutEffect(() => {
    positionRef.current = position;
    recordingRef.current = recording;
    surveyFrequencyRef.current = samples[0]?.frequencyHz;
    offsetRef.current = node.data.offset_hz;
    bandwidthRef.current = node.data.bandwidth_hz;
  });

  useEffect(
    () =>
      spectrumHub.subscribe(deviceSet, stream, (frame: SpectrumFrame) => {
        const targetHz = Math.round(frame.centerHz + offsetRef.current);
        const measured = measureSignalDbfs(frame, targetHz, bandwidthRef.current);
        const now = performance.now();
        if (now - lastLevelRenderRef.current >= LEVEL_REFRESH_MS || measured === null) {
          lastLevelRenderRef.current = now;
          setLive({ level: measured, targetHz, centerHz: frame.centerHz, spanHz: frame.spanHz });
        }

        const fix = positionRef.current;
        const surveyFrequency = surveyFrequencyRef.current;
        if (surveyFrequency !== undefined && targetHz !== surveyFrequency) {
          if (recordingRef.current) {
            recordingRef.current = false;
            setRecording(node.id, false);
          }
          return;
        }
        if (
          !recordingRef.current ||
          measured === null ||
          fix === undefined ||
          fix.receivedAt === lastRecordedRef.current
        ) {
          return;
        }
        lastRecordedRef.current = fix.receivedAt;
        observe(node.id, {
          latitude: fix.latitude,
          longitude: fix.longitude,
          frequencyHz: targetHz,
          levelDbfs: measured,
          measuredAt: Date.now(),
          ...(fix.accuracy_m == null ? {} : { accuracyM: fix.accuracy_m }),
        });
      }),
    [deviceSet, node.id, observe, setRecording, stream],
  );

  const updateSettings = (offsetHz: number, bandwidthHz: number): void => {
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, node.id, (current) =>
        current.kind === "signal_map"
          ? {
              ...current,
              data: { offset_hz: offsetHz, bandwidth_hz: bandwidthHz },
            }
          : current,
      ),
    }));
  };

  const level = live?.level ?? null;
  const surveyFrequency = samples[0]?.frequencyHz;
  const frequencyChanged =
    surveyFrequency !== undefined && live !== null && live.targetHz !== surveyFrequency;
  const offsetLimitHz =
    live === null
      ? MAX_OFFSET_HZ
      : Math.min(MAX_OFFSET_HZ, signalOffsetLimitHz(live.spanHz, node.data.bandwidth_hz));
  const status = surveyStatus(position, level, recording, frequencyChanged);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 flex-wrap items-end gap-2 border-b border-line bg-panel-2 p-2">
        <fieldset
          className="flex min-w-72 flex-1 flex-wrap items-center gap-2 disabled:opacity-60"
          disabled={samples.length > 0}
          title={samples.length > 0 ? "Clear the current survey before changing offset" : undefined}
        >
          <span className="legend">Offset (kHz)</span>
          <OffsetStepper
            offsetHz={node.data.offset_hz}
            limitHz={offsetLimitHz}
            centerHz={live?.centerHz ?? null}
            onOffset={(offset) => updateSettings(offset, node.data.bandwidth_hz)}
          />
        </fieldset>
        <label className={`${LABEL} flex w-28 flex-col items-stretch gap-1`}>
          Width (kHz)
          <Input
            key={node.data.bandwidth_hz}
            className={FIELD}
            defaultValue={`${node.data.bandwidth_hz / 1e3}`}
            inputMode="decimal"
            aria-label="Survey bandwidth in kilohertz"
            disabled={samples.length > 0}
            title={
              samples.length > 0 ? "Clear the current survey before changing bandwidth" : undefined
            }
            onBlur={(event) => {
              const bandwidth = Math.round(
                Number(event.currentTarget.value.replace(",", ".")) * 1e3,
              );
              if (!Number.isFinite(bandwidth) || bandwidth < 1 || bandwidth > 100_000_000) {
                event.currentTarget.value = `${node.data.bandwidth_hz / 1e3}`;
                return;
              }
              if (bandwidth !== node.data.bandwidth_hz) {
                const limit =
                  live === null
                    ? MAX_OFFSET_HZ
                    : Math.min(MAX_OFFSET_HZ, signalOffsetLimitHz(live.spanHz, bandwidth));
                const offset = Math.max(-limit, Math.min(limit, node.data.offset_hz));
                updateSettings(offset, bandwidth);
              }
            }}
          />
        </label>
        <Button
          type="button"
          className={recording ? BTN_DANGER : BTN_PRIMARY}
          disabled={!recording && (position === undefined || level === null || frequencyChanged)}
          aria-pressed={recording}
          onClick={() => setRecording(node.id, !recording)}
        >
          {recording ? "Pause" : "Start survey"}
        </Button>
        <Button
          type="button"
          className={clearArmed ? BTN_DANGER : BTN}
          disabled={samples.length === 0}
          onBlur={() => setClearArmed(false)}
          onClick={() => {
            if (!clearArmed) {
              setClearArmed(true);
              return;
            }
            clear(node.id);
            setClearArmed(false);
          }}
        >
          {clearArmed ? "Confirm clear" : "Clear"}
        </Button>
        <Button
          type="button"
          className={BTN}
          disabled={samples.length === 0}
          onClick={() => downloadSurvey(node, samples)}
        >
          Export CSV
        </Button>
      </div>
      <div className="flex shrink-0 items-center gap-3 border-b border-line px-2 py-1 font-mono text-[10px] tabular-nums">
        <span className={recording ? "text-accent" : "text-ink-dim"}>{status}</span>
        <span className="ml-auto text-ink-dim">{samples.length} cells</span>
        <span className="text-ink-dim">{live === null ? "— Hz" : formatHz(live.targetHz)}</span>
        <span
          className="min-w-20 text-right text-ink"
          title="Relative receiver level. Keep gain and antenna settings fixed when comparing locations."
        >
          {level === null ? "— dBFS" : `${level.toFixed(1)} dBFS`}
        </span>
      </div>
      <MapPanel
        kinds={[]}
        positionNodes={[positionNode]}
        signalSamples={samples}
        active={active}
        className="min-h-0 w-full flex-1"
      />
    </div>
  );
}

function surveyStatus(
  position: PositionSample | undefined,
  level: number | null,
  recording: boolean,
  frequencyChanged: boolean,
): string {
  if (position === undefined) {
    return "Waiting for GPS";
  }
  if (level === null) {
    return "Offset is outside the IQ span";
  }
  if (frequencyChanged) {
    return "IQ centre changed — clear to start a new survey";
  }
  return recording ? "Recording each new GPS fix" : "Ready";
}

function downloadSurvey(
  node: PatchNodeOf<"signal_map">,
  samples: Parameters<typeof signalSurveyCsv>[0],
): void {
  const blob = new Blob([signalSurveyCsv(samples, node.data.offset_hz, node.data.bandwidth_hz)], {
    type: "text/csv;charset=utf-8",
  });
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  const frequency = samples[0]?.frequencyHz ?? 0;
  link.download = `signal-survey-${frequency}-hz-${node.data.bandwidth_hz}-hz-wide.csv`;
  link.click();
  URL.revokeObjectURL(url);
}
