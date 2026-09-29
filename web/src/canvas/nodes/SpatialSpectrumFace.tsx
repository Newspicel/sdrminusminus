import { useState } from "react";
import { NumberField } from "../../components/NumberField";
import { ColourScale } from "../../components/PlotAxis";
import { COLOUR_OPTIONS } from "../../components/plotFrame";
import { Segmented } from "../../components/Segmented";
import { Select } from "../../components/Select";
import { SettingRow } from "../../components/Settings";
import { type Colormap, DEFAULT_COLORMAP } from "../../gl/surface";
import { processorStatusOf, useArrayStore } from "../../lib/arrays";
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
import { BandRows } from "./BandRows";
import { FaceBody, FaceEmpty, NodeShell } from "./NodeShell";
import { ProcessorChips, ProcessorFaults, ProcessorReadout, ReadoutCell } from "./ProcessorReadout";
import { ageLabel, NO_CATALOG, processorSubtitle, useProcessorEdit } from "./processorFace";
import { SettingsFold } from "./SettingsFold";
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
const SMALL = "w-24";

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
              known={processorStatusOf(status, node.id) !== null}
              view={view}
              bearingFrame={bearingFrame}
              offsetDeg={offsetDeg}
              colormap={colormap}
              peaks={reading?.peaks ?? []}
              dim={stale || reading === null}
            />
            <SpatialReadout
              reading={reading}
              bearingFrame={bearingFrame}
              offsetDeg={offsetDeg}
              receivedAt={state?.receivedAt}
              now={now}
            />
            <ProcessorFaults status={processorStatusOf(status, node.id)} />
            <div className="max-h-48 shrink-0 overflow-y-auto">
              <SpatialSettings
                settings={settings}
                edit={edit}
                colormap={colormap}
                onColormap={setColormap}
              />
            </div>
          </div>
        )}
      </FaceBody>
    </NodeShell>
  );
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
  const dropped = reading?.dropped_frames ?? 0;
  return (
    <>
      <div className="shrink-0 border-t border-line px-2 py-1.5">
        <ProcessorReadout>
          <ReadoutCell
            label="Peak"
            value={peaks[0] === undefined ? "-" : peakText(peaks[0], bearingFrame, offsetDeg)}
            wide
          />
          {peaks.slice(1).map((peak, rank) => (
            <ReadoutCell
              key={`${peak.freq_hz}:${peak.bearing_deg}`}
              label={`#${rank + 2}`}
              value={peakText(peak, bearingFrame, offsetDeg)}
              wide
            />
          ))}
          <ReadoutCell label="Age" value={ageLabel(receivedAt, now)} wide />
        </ProcessorReadout>
      </div>
      <ProcessorChips
        chips={
          dropped > 0 ? [{ label: `Drops ${dropped}`, title: "Frames lost", danger: true }] : []
        }
      />
    </>
  );
}

function SpatialSettings({
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
    <SettingsFold label="Settings">
      <SettingRow label="Method">
        <Select
          label="Method"
          value={settings.method}
          options={METHOD_OPTIONS}
          onChange={(method) => edit({ method })}
        />
      </SettingRow>
      <SettingRow label="FFT" title="Frequency bins">
        <Select
          label="FFT"
          value={settings.bins}
          options={BIN_OPTIONS}
          onChange={(bins) => edit({ bins, columns: Math.min(settings.columns, bins) })}
          className={SMALL}
        />
      </SettingRow>
      <SettingRow label="Columns" title="Frequency columns shown">
        <Select
          label="Columns"
          value={settings.columns}
          options={columnOptions(settings.bins)}
          onChange={(columns) => edit({ columns })}
          className={SMALL}
        />
      </SettingRow>
      <SettingRow label="Average">
        <NumberField
          label="Average"
          value={settings.average_ms}
          min={50}
          max={10_000}
          step={10}
          unit="ms"
          className={SMALL}
          onCommit={(ms) => edit({ average_ms: Math.round(ms) })}
        />
      </SettingRow>
      <SettingRow label="Rate" title="New picture this often">
        <NumberField
          label="Rate"
          value={settings.report_ms}
          min={50}
          max={2_000}
          step={10}
          unit="ms"
          className={SMALL}
          onCommit={(ms) => edit({ report_ms: Math.round(ms) })}
        />
      </SettingRow>
      <SettingRow label="Step" title="Bearing resolution">
        <Select
          label="Step"
          value={settings.azimuth_step_deg}
          options={STEP_OPTIONS}
          onChange={(azimuth_step_deg) => edit({ azimuth_step_deg })}
          className={SMALL}
        />
      </SettingRow>
      <SettingRow label="Range" title="Colour range below the peak">
        <NumberField
          label="Range"
          value={settings.span_db}
          min={10}
          max={80}
          step={1}
          unit="dB"
          className={SMALL}
          onCommit={(span_db) => edit({ span_db })}
        />
      </SettingRow>
      <BandRows
        offsetHz={settings.offset_hz}
        bandwidthHz={settings.bandwidth_hz ?? null}
        onOffset={(offset_hz) => edit({ offset_hz })}
        onBandwidth={(bandwidth_hz) => edit({ bandwidth_hz })}
      />
      <SettingRow label="Colours">
        <Select
          label="Colours"
          value={colormap}
          options={COLOUR_OPTIONS}
          onChange={onColormap}
          className="w-28"
        />
      </SettingRow>
    </SettingsFold>
  );
}
