import { useState } from "react";
import { Chips, ChoiceChip, NumberChip } from "../../components/face/Chips";
import { Readout, Readouts } from "../../components/face/Readouts";
import { ColourScale } from "../../components/PlotAxis";
import { COLOUR_OPTIONS } from "../../components/plotFrame";
import { Segmented } from "../../components/Segmented";
import { type Colormap, DEFAULT_COLORMAP } from "../../gl/surface";
import { processorStatusOf, useArrayStore } from "../../lib/arrays";
import { SPATIAL_LIMITS as LIMITS } from "../../lib/limits";
import { isStale, readingOf, useProcessorStore } from "../../lib/processors";
import type {
  PatchNode,
  PatchNodeOf,
  SpatialReading,
  SpatialSpectrumParams,
} from "../../lib/types";
import { useNow } from "../../lib/useNow";
import { arrayOf } from "../binding";
import { useWorkspaceContext } from "../context";
import { settingsOf } from "../newNode";
import { BandChips } from "./BandRows";
import { FaceBody, FaceEmpty, NodeShell } from "./NodeShell";
import { ProcessorFooter } from "./ProcessorFooter";
import { type Chip, ProcessorError } from "./ProcessorHealth";
import { ageLabel, NO_CATALOG, processorSubtitle, useProcessorEdit } from "./processorFace";
import { SpatialPlot } from "./SpatialPlot";
import {
  type BearingFrame,
  BIN_OPTIONS,
  columnOptions,
  frameOffset,
  frameOptions,
  METHOD_OPTIONS,
  peakText,
  type SpatialView,
  STEP_OPTIONS,
  topPeaks,
  VIEW_OPTIONS,
} from "./spatialSpectrum";

const AGE_TICK_MS = 1_000;

type Edit = (next: Partial<SpatialSpectrumParams>) => void;

export function SpatialSpectrumFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const state = useProcessorStore((store) => store.byNode[node.id]);
  const array = arrayOf(workspace.graph, node.id);
  const status = useArrayStore((store) => (array === null ? undefined : store.byNode[array]));
  const now = useNow(AGE_TICK_MS);
  const edit = useProcessorEdit(node as PatchNodeOf<"spatial_spectrum">);
  const [chosenFrame, setChosenFrame] = useState<BearingFrame | null>(null);
  const [view, setView] = useState<SpatialView>("map");
  const [colormap, setColormap] = useState<Colormap>(DEFAULT_COLORMAP);
  if (node.kind !== "spatial_spectrum") {
    return null;
  }
  const settings = settingsOf(node, workspace.context.catalog);
  const reading = readingOf(state, "spatial_spectrum");
  const period = settings?.report_ms ?? 0;
  const stale = state !== undefined && isStale(state.receivedAt, now, period);
  const trueKnown = reading?.azimuth_deg != null;
  const bearingFrame: BearingFrame = trueKnown ? (chosenFrame ?? "true") : "relative";
  const span = settings?.span_db ?? 0;
  const offsetDeg = frameOffset(bearingFrame, reading);
  const processor = processorStatusOf(status, node.id);
  return (
    <NodeShell
      node={node}
      title="Spatial spectrum"
      category="tool"
      subtitle={processorSubtitle(workspace.graph, node.id, status, state, now, period)}
    >
      <FaceBody scroll={false}>
        {settings === null ? (
          <FaceEmpty hint={NO_CATALOG} />
        ) : (
          <div className="flex min-h-0 flex-1 flex-col">
            <div className="flex shrink-0 flex-wrap items-center gap-2 px-2 py-1.5">
              <Segmented label="View" value={view} options={VIEW_OPTIONS} onChange={setView} />
              <Segmented
                label="Bearing frame"
                value={bearingFrame}
                options={frameOptions(trueKnown)}
                onChange={setChosenFrame}
              />
              {view === "map" && (
                <span className="ml-auto" title="Below the strongest cell">
                  <ColourScale colormap={colormap} min={-span} max={0} unit="dB" />
                </span>
              )}
            </div>
            <SpatialPlot
              node={node.id}
              known={processor !== null}
              view={view}
              bearingFrame={bearingFrame}
              offsetDeg={offsetDeg}
              colormap={colormap}
              peaks={reading?.peaks ?? []}
              dim={stale || reading === null}
            />
            <SpatialChips
              settings={settings}
              edit={edit}
              colormap={colormap}
              onColormap={setColormap}
            />
            <SpatialReadout
              reading={reading}
              bearingFrame={bearingFrame}
              offsetDeg={offsetDeg}
              receivedAt={state?.receivedAt}
              now={now}
            />
            <ProcessorError status={processor} />
          </div>
        )}
      </FaceBody>
      <ProcessorFooter status={processor} chips={droppedChips(reading)} />
    </NodeShell>
  );
}

function droppedChips(reading: SpatialReading | null): Chip[] {
  const dropped = reading?.dropped_frames ?? 0;
  return dropped > 0 ? [{ label: `Drops ${dropped}`, title: "Frames lost", danger: true }] : [];
}

function SpatialReadout({
  reading,
  bearingFrame,
  offsetDeg,
  receivedAt,
  now,
}: {
  reading: SpatialReading | null;
  bearingFrame: BearingFrame;
  offsetDeg: number;
  receivedAt: number | undefined;
  now: number;
}) {
  const peaks = topPeaks(reading);
  return (
    <Readouts>
      <Readout label="Peak">
        {peaks[0] === undefined ? "-" : peakText(peaks[0], bearingFrame, offsetDeg)}
      </Readout>
      {peaks.slice(1).map((peak, rank) => (
        <Readout key={`${peak.freq_hz}:${peak.bearing_deg}`} label={`#${rank + 2}`}>
          {peakText(peak, bearingFrame, offsetDeg)}
        </Readout>
      ))}
      <Readout label="Age">{ageLabel(receivedAt, now)}</Readout>
    </Readouts>
  );
}

function SpatialChips({
  settings,
  edit,
  colormap,
  onColormap,
}: {
  settings: SpatialSpectrumParams;
  edit: Edit;
  colormap: Colormap;
  onColormap: (colormap: Colormap) => void;
}) {
  return (
    <Chips className="shrink-0 p-2">
      <ChoiceChip
        label="Method"
        title="Method"
        value={settings.method}
        options={METHOD_OPTIONS}
        onChange={(method) => edit({ method })}
      />
      <ChoiceChip
        label="FFT"
        title="Frequency bins"
        value={settings.bins}
        options={BIN_OPTIONS}
        onChange={(bins) => edit({ bins, columns: Math.min(settings.columns, bins) })}
      />
      <ChoiceChip
        label="Columns"
        title="Frequency columns shown"
        value={settings.columns}
        options={columnOptions(settings.bins)}
        onChange={(columns) => edit({ columns })}
      />
      <NumberChip
        label="Average"
        title="Average"
        unit="ms"
        value={settings.average_ms}
        min={LIMITS.average_ms.min}
        max={LIMITS.average_ms.max}
        step={10}
        onCommit={(ms) => edit({ average_ms: Math.round(ms) })}
      />
      <NumberChip
        label="Rate"
        title="New picture this often"
        unit="ms"
        value={settings.report_ms}
        min={LIMITS.report_ms.min}
        max={LIMITS.report_ms.max}
        step={10}
        onCommit={(ms) => edit({ report_ms: Math.round(ms) })}
      />
      <ChoiceChip
        label="Step"
        title="Bearing resolution"
        value={settings.azimuth_step_deg}
        options={STEP_OPTIONS}
        onChange={(azimuth_step_deg) => edit({ azimuth_step_deg })}
      />
      <NumberChip
        label="Range"
        title="Colour range below the peak"
        unit="dB"
        value={settings.span_db}
        min={LIMITS.span_db.min}
        max={LIMITS.span_db.max}
        step={1}
        onCommit={(span_db) => edit({ span_db })}
      />
      <BandChips
        band={LIMITS.band}
        offsetHz={settings.offset_hz}
        bandwidthHz={settings.bandwidth_hz ?? null}
        onOffset={(offset_hz) => edit({ offset_hz })}
        onBandwidth={(bandwidth_hz) => edit({ bandwidth_hz })}
      />
      <ChoiceChip
        label="Colours"
        title="Colours"
        value={colormap}
        options={COLOUR_OPTIONS}
        onChange={onColormap}
      />
    </Chips>
  );
}
