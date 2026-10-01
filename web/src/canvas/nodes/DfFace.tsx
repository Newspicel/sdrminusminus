import { useState } from "react";
import { Checkbox } from "../../components/Checkbox";
import {
  ChipField,
  Chips,
  ChoiceChip,
  NumberChip,
  SettingChip,
  ToggleChip,
} from "../../components/face/Chips";
import { FoldSection } from "../../components/face/Fold";
import { Readout, Readouts } from "../../components/face/Readouts";
import { NumberField } from "../../components/NumberField";
import { Rose } from "../../components/Rose";
import { Segmented } from "../../components/Segmented";
import { SettingRow, Settings } from "../../components/Settings";
import { TextField } from "../../components/TextField";
import { processorStatusOf, useArrayStore } from "../../lib/arrays";
import { DF_LIMITS as LIMITS, scaled } from "../../lib/limits";
import { isStale, readingOf, useProcessorStore } from "../../lib/processors";
import type {
  ArrayGeometry,
  DfParams,
  DfReading,
  PatchGraph,
  PatchNode,
  PatchNodeOf,
} from "../../lib/types";
import { useNow } from "../../lib/useNow";
import { arrayOf } from "../binding";
import { useWorkspaceContext } from "../context";
import { arrayWiredLanes, nodeOf } from "../graph";
import { settingsOf } from "../newNode";
import { allowsStructured, isCollinear, isLevelLine } from "./arrayGeometry";
import {
  AUTO_SOURCES,
  algorithmOptions,
  type BearingFrame,
  dfChips,
  elevationBlock,
  frameOptions,
  frameRotation,
  PEAK_OPTIONS,
  peakSummary,
  peakText,
  percentText,
  RULE_OPTIONS,
  roseLetters,
  roseNeedles,
  roseWedge,
  SIDE_OPTIONS,
  shownFrame,
  sigmaText,
  smoothingTitle,
  sourceOptions,
} from "./df";
import { FaceBody, FaceEmpty, NodeShell } from "./NodeShell";
import { ProcessorFooter } from "./ProcessorFooter";
import { ProcessorError } from "./ProcessorHealth";
import {
  ageLabel,
  NO_CATALOG,
  processorGate,
  processorSubtitle,
  useProcessorEdit,
} from "./processorFace";

const AGE_TICK_MS = 1_000;
const ROSE_PX = 220;
const KHZ = 1_000;
const OFFSET_KHZ = scaled(LIMITS.band.offset_hz, 1 / KHZ);
const WIDTH_KHZ = scaled(LIMITS.band.bandwidth_hz, 1 / KHZ);

type Edit = (next: Partial<DfParams>) => void;

interface ArrayShape {
  geometry: ArrayGeometry | null;
  lanes: number;
}

function structuredOk(shape: ArrayShape): boolean {
  return shape.geometry === null || allowsStructured(shape.geometry, shape.lanes);
}

function arrayShape(graph: PatchGraph, array: string | null): ArrayShape {
  if (array === null) {
    return { geometry: null, lanes: 0 };
  }
  const node = nodeOf(graph, array);
  return {
    geometry: node?.kind === "array" ? node.data.geometry : null,
    lanes: arrayWiredLanes(graph, array),
  };
}

export function DfFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const state = useProcessorStore((store) => store.byNode[node.id]);
  const array = arrayOf(workspace.graph, node.id);
  const status = useArrayStore((store) => (array === null ? undefined : store.byNode[array]));
  const now = useNow(AGE_TICK_MS);
  const [picked, setPicked] = useState<BearingFrame | null>(null);
  const edit = useProcessorEdit(node as PatchNodeOf<"df">);
  if (node.kind !== "df") {
    return null;
  }
  const settings = settingsOf(node, workspace.context.catalog);
  const reportMs = settings?.report_ms ?? 0;
  const reading = readingOf(state, "df");
  const stale = state !== undefined && isStale(state.receivedAt, now, reportMs);
  const gate = processorGate(status, node.id);
  const trueAvailable = reading?.azimuth_deg != null;
  const frame = shownFrame(picked, trueAvailable);
  const peaks = reading?.peaks ?? [];
  const processor = processorStatusOf(status, node.id);
  return (
    <NodeShell
      node={node}
      title="Direction finder"
      category="tool"
      subtitle={processorSubtitle(workspace.graph, node.id, status, state, now, reportMs)}
    >
      <FaceBody>
        <div className="flex flex-col items-center gap-2 p-2">
          <Segmented
            label="Bearing frame"
            value={frame}
            options={frameOptions(trueAvailable)}
            onChange={setPicked}
          />
          <Rose
            label="Bearing rose"
            size={ROSE_PX}
            marks={roseLetters(frame === "true")}
            spectrum={reading?.pseudospectrum ?? []}
            rotateDeg={frameRotation(reading, frame)}
            needles={roseNeedles(peaks, frame)}
            wedge={roseWedge(peaks, frame)}
            tickDeg={frame === "true" ? (reading?.azimuth_deg ?? null) : null}
            dim={stale || gate !== null}
          />
        </div>
        {settings === null ? (
          <FaceEmpty hint={NO_CATALOG} />
        ) : (
          <DfChips settings={settings} edit={edit} shape={arrayShape(workspace.graph, array)} />
        )}
        <DfReadout reading={reading} frame={frame} age={ageLabel(state?.receivedAt, now)} />
        <ProcessorError status={processor} />
        {settings !== null && (
          <FoldSection label="More">
            <DfMoreSettings
              settings={settings}
              edit={edit}
              shape={arrayShape(workspace.graph, array)}
            />
          </FoldSection>
        )}
      </FaceBody>
      <ProcessorFooter status={processor} chips={dfChips(reading, gate, stale)} />
    </NodeShell>
  );
}

function DfReadout({
  reading,
  frame,
  age,
}: {
  reading: DfReading | null;
  frame: BearingFrame;
  age: string;
}) {
  const peaks = reading?.peaks ?? [];
  const first = peaks[0];
  const others = peaks.slice(1).map((peak, index) => ({ rank: index + 2, peak }));
  return (
    <Readouts columns={2}>
      <Readout label="Bearing">{first === undefined ? "-" : peakText(first, frame)}</Readout>
      <Readout label="±" title="One sigma">
        {first === undefined ? "-" : sigmaText(first.sigma_deg)}
      </Readout>
      <Readout label="Fit" title="Share of the signal the bearings explain">
        {reading === null ? "-" : percentText(reading.fit ?? 0)}
      </Readout>
      <Readout label="SNR">
        {reading === null ? "-" : `${Math.round(reading.snr_db ?? 0)} dB`}
      </Readout>
      <Readout label="Src" title={reading?.sources_auto === false ? "Fixed" : "Auto"}>
        {reading === null ? "-" : String(reading.sources)}
      </Readout>
      <Readout label="Age">{age}</Readout>
      {others.map(({ rank, peak }) => (
        <Readout key={rank} label={`#${rank}`} wide>
          {peakSummary(peak, frame)}
        </Readout>
      ))}
    </Readouts>
  );
}

function DfChips({ settings, edit, shape }: { settings: DfParams; edit: Edit; shape: ArrayShape }) {
  const collinear = shape.geometry !== null && isCollinear(shape.geometry, shape.lanes);
  const line = shape.geometry !== null && isLevelLine(shape.geometry, shape.lanes);
  const elevationRefused = elevationBlock(collinear, settings.algorithm, settings.smoothing);
  return (
    <Chips className="p-2">
      <ChoiceChip
        label="Method"
        title="Method"
        value={settings.algorithm}
        options={algorithmOptions(structuredOk(shape))}
        onChange={(algorithm) => edit({ algorithm })}
      />
      <NumberChip
        label="Offset"
        title="Signal offset from the array centre"
        unit="kHz"
        value={settings.offset_hz / KHZ}
        min={OFFSET_KHZ.min}
        max={OFFSET_KHZ.max}
        step={0.1}
        quiet={settings.offset_hz === 0}
        onCommit={(khz) => edit({ offset_hz: Math.round(khz * KHZ) })}
      />
      <NumberChip
        label="Width"
        title="Band the finder listens to"
        unit="kHz"
        value={settings.bandwidth_hz / KHZ}
        min={WIDTH_KHZ.min}
        max={WIDTH_KHZ.max}
        step={0.1}
        onCommit={(khz) => edit({ bandwidth_hz: Math.round(khz * KHZ) })}
      />
      <ChoiceChip
        label="Sources"
        title="Sources"
        value={settings.sources ?? AUTO_SOURCES}
        options={sourceOptions(shape.lanes, settings.sources ?? null)}
        quiet={settings.sources == null}
        onChange={(count) => edit({ sources: count === AUTO_SOURCES ? null : count })}
      />
      <ChoiceChip
        label="Rule"
        title="How Auto counts sources"
        value={settings.source_rule}
        options={RULE_OPTIONS}
        onChange={(source_rule) => edit({ source_rule })}
      />
      <ChoiceChip
        label="Peaks"
        title="Most bearings per report"
        value={settings.max_peaks}
        options={PEAK_OPTIONS}
        onChange={(max_peaks) => edit({ max_peaks })}
      />
      <NumberChip
        label="Squelch"
        title="Minimum peak above the floor, 0 off"
        unit="dB"
        shown={settings.squelch_db === 0 ? "off" : undefined}
        value={settings.squelch_db}
        min={LIMITS.squelch_db.min}
        max={LIMITS.squelch_db.max}
        step={0.5}
        quiet={settings.squelch_db === 0}
        onCommit={(squelch_db) => edit({ squelch_db })}
      />
      <NumberChip
        label="Report"
        title="Report"
        unit="ms"
        value={settings.report_ms}
        min={LIMITS.report_ms.min}
        max={LIMITS.report_ms.max}
        step={100}
        onCommit={(report_ms) => edit({ report_ms })}
      />
      <ToggleChip
        label="Elevation"
        title={elevationRefused ?? "Estimate elevation too"}
        on={settings.elevation}
        disabled={elevationRefused !== null && !settings.elevation}
        onChange={(elevation) => edit({ elevation })}
      />
      {line && (
        <ChoiceChip
          label="Side"
          title="Which side of the line to report"
          value={settings.ula_side}
          options={SIDE_OPTIONS}
          onChange={(ula_side) => edit({ ula_side })}
        />
      )}
      <StationChip settings={settings} edit={edit} />
    </Chips>
  );
}

function StationChip({ settings, edit }: { settings: DfParams; edit: Edit }) {
  return (
    <SettingChip
      label="Station"
      value={settings.station_id ?? "unnamed"}
      quiet={settings.station_id == null}
      title="Name on the bearings this finder sends"
    >
      {() => (
        <ChipField label="Station">
          <TextField
            label="Station"
            placeholder="unnamed"
            maxLength={LIMITS.station_len}
            value={settings.station_id ?? ""}
            onCommit={(station) => edit({ station_id: station === "" ? null : station })}
          />
        </ChipField>
      )}
    </SettingChip>
  );
}

function DfMoreSettings({
  settings,
  edit,
  shape,
}: {
  settings: DfParams;
  edit: Edit;
  shape: ArrayShape;
}) {
  const structured = structuredOk(shape);
  return (
    <Settings>
      <SettingRow label="FB" title="Forward-backward averaging, needs a symmetric array">
        <Checkbox
          label="Forward-backward averaging"
          checked={settings.forward_backward}
          onChange={(forward_backward) => edit({ forward_backward })}
        />
      </SettingRow>
      <SettingRow label="Smooth" title={smoothingTitle(structured)}>
        <NumberField
          label="Smooth"
          value={settings.smoothing}
          min={LIMITS.smoothing.min}
          max={LIMITS.smoothing.max}
          step={1}
          disabled={!structured && settings.smoothing === 0}
          onCommit={(smoothing) => edit({ smoothing })}
        />
      </SettingRow>
      <SettingRow label="Loading" title="Diagonal loading, steadies Capon and MUSIC">
        <NumberField
          label="Loading"
          value={settings.loading}
          min={LIMITS.loading.min}
          max={LIMITS.loading.max}
          step={0.001}
          onCommit={(loading) => edit({ loading })}
        />
      </SettingRow>
      <SettingRow label="Step" title="Scan grid">
        <NumberField
          label="Step"
          unit="°"
          value={settings.azimuth_step_deg}
          min={LIMITS.azimuth_step_deg.min}
          max={LIMITS.azimuth_step_deg.max}
          step={0.25}
          onCommit={(azimuth_step_deg) => edit({ azimuth_step_deg })}
        />
      </SettingRow>
      <SettingRow label="Yaw gate" title="Skip blocks while the array turns faster">
        <NumberField
          label="Yaw gate"
          unit="°/s"
          value={settings.yaw_gate_dps}
          min={LIMITS.yaw_gate_dps.min}
          max={LIMITS.yaw_gate_dps.max}
          step={1}
          onCommit={(yaw_gate_dps) => edit({ yaw_gate_dps })}
        />
      </SettingRow>
      <SettingRow label="Carry over" title="Share of the last report kept">
        <NumberField
          label="Carry over"
          value={settings.carry_over}
          min={LIMITS.carry_over.min}
          max={LIMITS.carry_over.max}
          step={0.01}
          onCommit={(carry_over) => edit({ carry_over })}
        />
      </SettingRow>
    </Settings>
  );
}
