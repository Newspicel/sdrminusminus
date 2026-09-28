import type { GeoJSONSource } from "maplibre-gl";
import { type RefObject, useEffect } from "react";
import { paletteTable } from "../../gl/surface";
import { useDecodedStore } from "../../lib/decoded";
import type { FusionGridFrame } from "../../lib/frame";
import { advanceFixTrails, type DfOverlay, drawDfOverlay, type FixTrails } from "../../lib/map/df";
import { drawHeat, type HeatScratch } from "../../lib/map/heat";
import {
  DRAW_TICK_MS,
  type MapKind,
  sourceId,
  type Target,
  type TargetDetail,
  targetCollection,
  targetDetail,
} from "../../lib/map/layers";
import { POSITION_ROUTE_SOURCE, POSITION_SOURCE } from "../../lib/map/position";
import { type PropagationOverlay, updatePropagationSources } from "../../lib/map/propagation";
import { SIGNAL_SOURCE } from "../../lib/map/signal";
import {
  framePositionOnce,
  frameSignalOnce,
  frameTargetsOnce,
  setSourceData,
  updatePositionSources,
  updateSignalSource,
} from "../../lib/map/sources";
import { highlight } from "../../lib/map/targets";
import { type PositionSample, usePositionStore } from "../../lib/position";
import type { SignalSurveySample } from "../../lib/signalSurvey";
import {
  type Counts,
  type MapCore,
  type MapInputs,
  type MapSinks,
  readyCore,
  sameCounts,
  ZERO_COUNTS,
} from "./mapState";

const HEAT_COLORMAP = "inferno";
const EMPTY_STATIONS: readonly Target[] = Object.freeze([]);
const EMPTY_HISTORY: readonly PositionSample[] = Object.freeze([]);

interface Drawn {
  generation: number;
  targets: Partial<Record<MapKind, readonly Target[]>>;
  position: string;
  signal: readonly SignalSurveySample[] | null;
  propagation: PropagationOverlay | null;
  df: DfOverlay | null;
  heat: FusionGridFrame | null;
  trails: FixTrails;
  counts: Counts;
}

interface Painter {
  table: Uint8ClampedArray;
  scratch: HeatScratch;
}

function freshDrawn(generation: number, counts: Counts, trails: FixTrails): Drawn {
  return {
    generation,
    targets: {},
    position: "",
    signal: null,
    propagation: null,
    df: null,
    heat: null,
    trails,
    counts,
  };
}

function findDetail(kind: MapKind, id: string): TargetDetail | null {
  const station = useDecodedStore.getState().stations[kind]?.find((row) => row.id === id);
  return station === undefined ? null : targetDetail(station);
}

function drawPositions(core: MapCore, inputs: MapInputs, drawn: Drawn, sinks: MapSinks): void {
  const sources = usePositionStore.getState().sources;
  const tracks = inputs.positionNodes.map((node) => ({
    node,
    samples: sources[node]?.history ?? EMPTY_HISTORY,
    active: sources[node]?.fix != null,
  }));
  const key = tracks
    .map(
      ({ node, samples, active }) =>
        `${node}:${samples.length}:${samples.at(-1)?.receivedAt ?? 0}:${active ? "live" : "stale"}`,
    )
    .join("|");
  if (key === drawn.position) {
    return;
  }
  drawn.position = key;
  const collection = updatePositionSources(
    core.map.getSource<GeoJSONSource>(POSITION_SOURCE),
    core.map.getSource<GeoJSONSource>(POSITION_ROUTE_SOURCE),
    tracks,
  );
  sinks.setPositionCount(collection.points.features.length);
  framePositionOnce(core.map, collection.points, core.framing.position);
}

function drawOverlays(core: MapCore, inputs: MapInputs, drawn: Drawn, painter: Painter): void {
  const propagation = inputs.propagation;
  if (propagation !== null && propagation !== drawn.propagation) {
    drawn.propagation = propagation;
    updatePropagationSources(core.map, propagation);
  }
  const df = inputs.df;
  if (df !== null && df !== drawn.df) {
    drawn.df = df;
    drawn.trails = advanceFixTrails(drawn.trails, df.tracks);
    drawDfOverlay(core.map, df, drawn.trails);
  }
  const heat = inputs.heat;
  if (heat !== null && heat !== drawn.heat) {
    drawn.heat = heat;
    drawHeat(core.map, heat, painter.table, painter.scratch);
  }
  const samples = inputs.signalSamples;
  if (samples !== null && samples !== drawn.signal) {
    drawn.signal = samples;
    const source = core.map.getSource<GeoJSONSource>(SIGNAL_SOURCE);
    frameSignalOnce(core.map, updateSignalSource(source, samples), core.framing.signal);
  }
}

function drawTargets(core: MapCore, inputs: MapInputs, drawn: Drawn, sinks: MapSinks): void {
  const stations = useDecodedStore.getState().stations;
  const now = Date.now();
  const next = { ...drawn.counts };
  for (const kind of inputs.kinds) {
    const rows = stations[kind] ?? EMPTY_STATIONS;
    if (rows === drawn.targets[kind]) {
      continue;
    }
    drawn.targets[kind] = rows;
    const collection = targetCollection(rows, now);
    setSourceData(core.map.getSource<GeoJSONSource>(sourceId(kind)), collection);
    next[kind] = collection.features.length;
    frameTargetsOnce(core.map, collection, core.framing.targets);
  }
  if (!sameCounts(drawn.counts, next)) {
    drawn.counts = next;
    sinks.setCounts(next);
  }
}

function followSelection(core: MapCore, inputs: MapInputs, sinks: MapSinks): void {
  const selected = core.selected;
  if (selected === null) {
    return;
  }
  const current = findDetail(selected.kind, selected.id);
  sinks.setDetail((previous) =>
    current === null || previous === null || current.lastSeen !== previous.lastSeen
      ? current
      : previous,
  );
  if (current === null) {
    core.selected = null;
    highlight(core.map, inputs.kinds, null);
  }
}

function drawFrame(
  core: MapCore,
  inputs: MapInputs,
  drawn: Drawn,
  painter: Painter,
  sinks: MapSinks,
): void {
  drawPositions(core, inputs, drawn, sinks);
  drawOverlays(core, inputs, drawn, painter);
  drawTargets(core, inputs, drawn, sinks);
  followSelection(core, inputs, sinks);
}

export function useMapDrawLoop(
  coreRef: RefObject<MapCore | null>,
  inputsRef: RefObject<MapInputs>,
  sinks: MapSinks,
): void {
  useEffect(() => {
    const painter: Painter = { table: paletteTable(HEAT_COLORMAP), scratch: { pixels: null } };
    let drawn = freshDrawn(-1, ZERO_COUNTS, new Map());
    const timer = setInterval(() => {
      const core = readyCore(coreRef.current);
      if (core === null) {
        return;
      }
      if (drawn.generation !== core.generation) {
        drawn = freshDrawn(core.generation, drawn.counts, drawn.trails);
      }
      drawFrame(core, inputsRef.current, drawn, painter, sinks);
    }, DRAW_TICK_MS);
    return () => clearInterval(timer);
  }, [coreRef, inputsRef, sinks]);
}
