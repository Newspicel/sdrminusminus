import { useState } from "react";
import { Button, Input } from "../../components/BaseControls";
import { BTN, BTN_DANGER, BTN_PRIMARY, FIELD, LABEL } from "../../components/controls";
import { formatHz, formatSignedHz } from "../../components/format";
import { MapPanel } from "../../components/MapPanel";
import { OffsetStepper } from "../../components/OffsetStepper";
import { FieldUnitFrame, unitPadding } from "../../components/Unit";
import { positionSourcesOf, usePositionStore } from "../../lib/position";
import {
  canStart,
  retunedSince,
  type SurveyView,
  signalOffsetLimitHz,
  signalSurveyCsv,
  surveyFrequencyHz,
  surveyStatus,
} from "../../lib/signalSurvey";
import { EMPTY_SURVEY, useSurveyControl, useSurveySeed, useSurveyStore } from "../../lib/survey";
import type { PatchNode, PatchNodeOf, SurveyCell } from "../../lib/types";
import { hasWire, iqSourceOf } from "../binding";
import { useWorkspaceContext, type Workspace } from "../context";
import { patchNode } from "../graph";
import { deviceSetOf } from "../workspaceDevice";
import { laneCenterHz, laneRateHz } from "./deviceNode";
import { FaceBody, NodeShell, useFaceActive } from "./NodeShell";

const NO_POSITIONS: readonly string[] = [];
const MAX_OFFSET_HZ = 1_000_000_000_000;
const MAX_BANDWIDTH_HZ = 100_000_000;

export function SignalMapFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const set = deviceSetOf(workspace, node.id);
  const iq = iqSourceOf(workspace.graph, node.id, workspace.devices);
  const positionNode = positionSourcesOf(workspace.graph, node.id)[0] ?? null;
  const positioned = usePositionStore(
    (store) => positionNode !== null && (store.sources[positionNode]?.fix ?? null) !== null,
  );

  if (node.kind !== "signal_map") {
    return null;
  }

  return (
    <NodeShell
      node={node}
      title="Signal survey"
      category="output"
      subtitle={set === null ? undefined : formatSignedHz(node.data.offset_hz)}
    >
      <FaceBody scroll={false}>
        <SignalSurvey
          node={node}
          centerHz={set === null || iq === null ? null : laneCenterHz(set, iq.stream)}
          spanHz={set === null || iq === null ? null : (laneRateHz(set, iq.stream) ?? null)}
          radioWired={hasWire(workspace.graph, node.id, "iq")}
          positionNode={positionNode}
          positioned={positioned}
        />
      </FaceBody>
    </NodeShell>
  );
}

export function editSurveyBand(
  workspace: Pick<Workspace, "edit" | "apply">,
  node: string,
  offsetHz: number,
  bandwidthHz: number,
): void {
  workspace.edit((snapshot) => ({
    ...snapshot,
    graph: patchNode(snapshot.graph, node, (current) =>
      current.kind === "signal_map"
        ? { ...current, data: { offset_hz: offsetHz, bandwidth_hz: bandwidthHz } }
        : current,
    ),
  }));
  workspace.apply();
}

function offsetLimitHz(spanHz: number | null, bandwidthHz: number): number {
  return spanHz === null
    ? MAX_OFFSET_HZ
    : Math.min(MAX_OFFSET_HZ, signalOffsetLimitHz(spanHz, bandwidthHz));
}

function SignalSurvey({
  node,
  centerHz,
  spanHz,
  radioWired,
  positionNode,
  positioned,
}: {
  node: PatchNodeOf<"signal_map">;
  centerHz: number | null;
  spanHz: number | null;
  radioWired: boolean;
  positionNode: string | null;
  positioned: boolean;
}) {
  const workspace = useWorkspaceContext();
  const active = useFaceActive();
  useSurveySeed(node.id);
  const survey = useSurveyStore((store) => store.byNode[node.id] ?? EMPTY_SURVEY);
  const { control, pending } = useSurveyControl(node.id);
  const [clearArmed, setClearArmed] = useState(false);
  const cells = survey.cells;
  const held = cells.length > 0;
  const targetHz = radioWired ? survey.targetHz : null;
  const levelDbfs = radioWired ? survey.levelDbfs : null;
  const view: SurveyView = {
    radioWired,
    positionWired: positionNode !== null,
    positioned,
    targetHz,
    levelDbfs,
    recording: survey.recording,
    retuned: retunedSince(cells, targetHz),
  };

  const updateSettings = (offsetHz: number, bandwidthHz: number): void =>
    editSurveyBand(workspace, node.id, offsetHz, bandwidthHz);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 flex-wrap items-end gap-2 border-b border-line bg-panel-2 p-2">
        <fieldset
          className="flex min-w-72 flex-1 flex-wrap items-center gap-2 disabled:opacity-60"
          disabled={held}
          title={held ? "Clear the survey to change the offset" : undefined}
        >
          <span className="legend">Offset</span>
          <OffsetStepper
            offsetHz={node.data.offset_hz}
            limitHz={offsetLimitHz(spanHz, node.data.bandwidth_hz)}
            centerHz={centerHz}
            onOffset={(offset) => updateSettings(offset, node.data.bandwidth_hz)}
          />
        </fieldset>
        <BandwidthField
          node={node}
          held={held}
          onCommit={(bandwidth) => {
            const limit = offsetLimitHz(spanHz, bandwidth);
            updateSettings(Math.max(-limit, Math.min(limit, node.data.offset_hz)), bandwidth);
          }}
        />
        <Button
          type="button"
          className={survey.recording ? BTN_DANGER : BTN_PRIMARY}
          disabled={pending || (!survey.recording && !canStart(view))}
          aria-pressed={survey.recording}
          onClick={() => control(survey.recording ? "stop" : "start")}
        >
          {survey.recording ? "Pause" : "Start survey"}
        </Button>
        <Button
          type="button"
          className={clearArmed ? BTN_DANGER : BTN}
          disabled={pending || !held}
          onBlur={() => setClearArmed(false)}
          onClick={() => {
            if (clearArmed) {
              control("clear");
            }
            setClearArmed(!clearArmed);
          }}
        >
          {clearArmed ? "Confirm clear" : "Clear"}
        </Button>
        <Button
          type="button"
          className={BTN}
          disabled={!held}
          onClick={() => downloadSurvey(node, cells)}
        >
          Export CSV
        </Button>
      </div>
      <div className="flex shrink-0 items-center gap-3 border-b border-line px-2 py-1 font-mono text-[10px] tabular-nums">
        <span className={survey.recording ? "text-accent" : "text-ink-dim"}>
          {surveyStatus(view)}
        </span>
        <span className="ml-auto text-ink-dim">{cells.length} cells</span>
        {survey.dropped > 0 && (
          <span className="text-warn" title="Oldest cells dropped to stay within the limit">
            {survey.dropped} dropped
          </span>
        )}
        <span className="text-ink-dim">{targetHz === null ? "- Hz" : formatHz(targetHz)}</span>
        <span
          className="min-w-20 text-right text-ink"
          title="Relative receiver level. Keep gain and antenna settings fixed when comparing locations."
        >
          {levelDbfs === null ? "- dBFS" : `${levelDbfs.toFixed(1)} dBFS`}
        </span>
      </div>
      <MapPanel
        kinds={[]}
        positionNodes={positionNode === null ? NO_POSITIONS : [positionNode]}
        signalSamples={cells}
        active={active}
        className="min-h-0 w-full flex-1"
      />
    </div>
  );
}

function BandwidthField({
  node,
  held,
  onCommit,
}: {
  node: PatchNodeOf<"signal_map">;
  held: boolean;
  onCommit: (bandwidthHz: number) => void;
}) {
  return (
    <label className={`${LABEL} flex w-28 flex-col items-stretch gap-1`}>
      Width
      <FieldUnitFrame symbol="kHz">
        <Input
          key={node.data.bandwidth_hz}
          className={`${FIELD} w-full`}
          style={unitPadding("kHz")}
          defaultValue={`${node.data.bandwidth_hz / 1e3}`}
          inputMode="decimal"
          aria-label="Survey bandwidth"
          disabled={held}
          title={held ? "Clear the survey to change the width" : undefined}
          onBlur={(event) => {
            const bandwidth = Math.round(Number(event.currentTarget.value.replace(",", ".")) * 1e3);
            if (!Number.isFinite(bandwidth) || bandwidth < 1 || bandwidth > MAX_BANDWIDTH_HZ) {
              event.currentTarget.value = `${node.data.bandwidth_hz / 1e3}`;
              return;
            }
            if (bandwidth !== node.data.bandwidth_hz) {
              onCommit(bandwidth);
            }
          }}
        />
      </FieldUnitFrame>
    </label>
  );
}

function downloadSurvey(node: PatchNodeOf<"signal_map">, cells: readonly SurveyCell[]): void {
  const blob = new Blob([signalSurveyCsv(cells, node.data.offset_hz, node.data.bandwidth_hz)], {
    type: "text/csv;charset=utf-8",
  });
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  const frequency = surveyFrequencyHz(cells) ?? 0;
  link.download = `signal-survey-${frequency}-hz-${node.data.bandwidth_hz}-hz-wide.csv`;
  link.click();
  URL.revokeObjectURL(url);
}
