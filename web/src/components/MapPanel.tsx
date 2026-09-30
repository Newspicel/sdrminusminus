import "maplibre-gl/dist/maplibre-gl.css";
import { useLayoutEffect, useMemo, useRef, useState } from "react";
import { useDecodedStore } from "../lib/decoded";
import type { FusionGridFrame } from "../lib/frame";
import { type DfOverlay, overlayCounts } from "../lib/map/df";
import { type MapKind, type TargetDetail, targetDetail } from "../lib/map/layers";
import type { PropagationOverlay } from "../lib/map/propagation";
import { highlight, type Selection } from "../lib/map/targets";
import type { SurveyCell } from "../lib/types";
import { MapLegend } from "./map/MapLegend";
import { type Counts, type MapInputs, type MapSinks, ZERO_COUNTS } from "./map/mapState";
import { TargetCard } from "./map/TargetCard";
import { useMapDrawLoop } from "./map/useMapDrawLoop";
import { useLayerSync, useMapActive, useMapInstance } from "./map/useMapInstance";

function detailOf(selection: Selection | null): TargetDetail | null {
  if (selection === null) {
    return null;
  }
  const station = useDecodedStore
    .getState()
    .stations[selection.kind]?.find((row) => row.id === selection.id);
  return station === undefined ? null : targetDetail(station);
}

export function MapPanel({
  kinds,
  references = [],
  positionNodes = [],
  signalSamples,
  propagation,
  df,
  heat,
  heatRefused = null,
  active = true,
  className,
}: {
  kinds: readonly MapKind[];
  references?: readonly (readonly [number, number])[];
  positionNodes?: readonly string[];
  signalSamples?: readonly SurveyCell[];
  propagation?: PropagationOverlay;
  df?: DfOverlay;
  heat?: FusionGridFrame | null;
  heatRefused?: string | null;
  active?: boolean;
  className?: string;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const inputsRef = useRef<MapInputs>({
    kinds,
    references,
    positionNodes,
    signalSamples: signalSamples ?? null,
    propagation: propagation ?? null,
    df: df ?? null,
    heat: heat ?? null,
    heatEnabled: heat !== undefined,
  });
  useLayoutEffect(() => {
    inputsRef.current = {
      kinds,
      references,
      positionNodes,
      signalSamples: signalSamples ?? null,
      propagation: propagation ?? null,
      df: df ?? null,
      heat: heat ?? null,
      heatEnabled: heat !== undefined,
    };
  });
  const [counts, setCounts] = useState<Counts>(ZERO_COUNTS);
  const [positionCount, setPositionCount] = useState(0);
  const [detail, setDetail] = useState<TargetDetail | null>(null);
  const [headings, setHeadings] = useState(true);
  const sinks = useMemo<MapSinks>(
    () => ({ setCounts, setPositionCount, setDetail, setHeadings }),
    [],
  );
  const { coreRef, basemap } = useMapInstance(containerRef, inputsRef, sinks, (hit) =>
    setDetail(detailOf(hit)),
  );
  const close = () => {
    const core = coreRef.current;
    if (core !== null) {
      core.selected = null;
      highlight(core.map, kinds, null);
    }
    setDetail(null);
  };
  useLayerSync(
    coreRef,
    {
      kinds,
      references,
      positionNodes,
      signalEnabled: signalSamples !== undefined,
      propagationLayer: propagation?.layer ?? null,
      dfEnabled: df !== undefined,
      heatEnabled: heat !== undefined,
    },
    sinks,
  );
  useMapDrawLoop(coreRef, inputsRef, sinks);
  useMapActive(containerRef, coreRef, active, basemap);

  return (
    <div className={`relative ${className ?? "h-[min(60dvh,28rem)] min-h-64 w-full"}`}>
      <div ref={containerRef} className="h-full w-full bg-bg" />
      <MapLegend
        kinds={kinds}
        counts={counts}
        positionCount={positionNodes.length > 0 ? positionCount : null}
        signalCells={signalSamples === undefined ? null : signalSamples.length}
        overlay={overlayCounts(df)}
        heat={heat != null}
        heatRefused={heatRefused}
        headings={headings}
        basemap={basemap}
      />
      {detail !== null && <TargetCard detail={detail} onClose={close} />}
    </div>
  );
}
