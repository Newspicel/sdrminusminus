import type { Map as MapLibreMap } from "maplibre-gl";
import type { FusionGridFrame } from "../../lib/frame";
import type { DfOverlay } from "../../lib/map/df";
import type { MapKind, TargetDetail } from "../../lib/map/layers";
import type { PropagationOverlay } from "../../lib/map/propagation";
import type { Selection } from "../../lib/map/targets";
import type { SurveyCell } from "../../lib/types";

export type Counts = Record<MapKind, number>;

export const ZERO_COUNTS: Counts = { adsb: 0, ais: 0, aprs: 0 };

export interface MapInputs {
  kinds: readonly MapKind[];
  references: readonly (readonly [number, number])[];
  positionNodes: readonly string[];
  signalSamples: readonly SurveyCell[] | null;
  propagation: PropagationOverlay | null;
  df: DfOverlay | null;
  heat: FusionGridFrame | null;
  heatEnabled: boolean;
}

export interface FrameFlag {
  current: boolean;
}

export interface MapCore {
  map: MapLibreMap;
  ready: boolean;
  generation: number;
  edge: string;
  accent: string;
  framing: { targets: FrameFlag; position: FrameFlag; signal: FrameFlag };
  selected: Selection | null;
}

export interface MapSinks {
  setCounts: (counts: Counts) => void;
  setPositionCount: (count: number) => void;
  setDetail: (update: (previous: TargetDetail | null) => TargetDetail | null) => void;
  setHeadings: (ok: boolean) => void;
}

export function sameCounts(a: Counts, b: Counts): boolean {
  return a.adsb === b.adsb && a.ais === b.ais && a.aprs === b.aprs;
}

export function readyCore(core: MapCore | null): MapCore | null {
  return core !== null && core.ready ? core : null;
}
