import { useState } from "react";
import { GridCanvas } from "../../components/GridCanvas";
import { recordEvent } from "../../lib/diagnostics";
import type { FusionGridFrame } from "../../lib/frame";
import { STATION_COLOR } from "../../lib/map/df";
import type { DfEstimate, DfStation } from "../../lib/types";
import { useBoxSize } from "../../lib/useBoxSize";
import { useSurface } from "../../lib/useSurface";
import { FaceEmpty } from "./NodeShell";
import { type Box, type GridBounds, geoToGrid, insideBox } from "./triangulation";

const HEAT_PX = 180;
const CROSS = 6;
const STATION_R = 3.5;

export function FusionHeat({
  node,
  known,
  estimate,
  emitters,
  stations,
  hint,
}: {
  node: string;
  known: boolean;
  estimate: DfEstimate | null;
  emitters: readonly DfEstimate[];
  stations: readonly DfStation[];
  hint: string | null;
}) {
  const [frame, setFrame] = useState<FusionGridFrame | null>(null);
  const [ref, size] = useBoxSize<HTMLDivElement>();
  useSurface(known ? node : null, (surface) => {
    if (surface.kind === "fusion_grid") {
      setFrame(surface.frame);
    } else {
      recordEvent("warn", "triangulation", `unexpected ${surface.kind} surface`);
    }
  });
  const box = { w: size.width, h: size.height };
  return (
    <div
      ref={ref}
      className="relative shrink-0 bg-plot-bg"
      style={{ height: HEAT_PX }}
      role="img"
      aria-label="Bearing heat"
    >
      {frame === null ? (
        <div className="absolute inset-0 flex">
          <FaceEmpty hint={hint ?? "No bearings yet"} />
        </div>
      ) : (
        <>
          <GridCanvas frame={frame} colormap="inferno" flipY={false} />
          {box.w > 0 && box.h > 0 && (
            <HeatMarks
              bounds={frame}
              box={box}
              estimate={estimate}
              emitters={emitters}
              stations={stations}
            />
          )}
        </>
      )}
    </div>
  );
}

export function HeatMarks({
  bounds,
  box,
  estimate,
  emitters,
  stations,
}: {
  bounds: GridBounds;
  box: Box;
  estimate: DfEstimate | null;
  emitters: readonly DfEstimate[];
  stations: readonly DfStation[];
}) {
  const others = emitters.filter(
    (emitter) => estimate === null || emitter.lat !== estimate.lat || emitter.lon !== estimate.lon,
  );
  return (
    <svg
      aria-hidden
      className="pointer-events-none absolute inset-0 size-full"
      viewBox={`0 0 ${box.w} ${box.h}`}
    >
      {stations.map((station) => {
        const at = geoToGrid(station, bounds, box);
        return insideBox(at, box) ? (
          <circle
            key={station.station_id}
            cx={at.x}
            cy={at.y}
            r={STATION_R}
            fill={STATION_COLOR}
            className="stroke-plot-bg"
          >
            <title>{station.station_id}</title>
          </circle>
        ) : null;
      })}
      {others.map((emitter) => (
        <Cross
          key={`${emitter.lat},${emitter.lon}`}
          at={geoToGrid(emitter, bounds, box)}
          box={box}
          className="stroke-accent/50"
        />
      ))}
      {estimate !== null && (
        <Cross at={geoToGrid(estimate, bounds, box)} box={box} className="stroke-accent stroke-2" />
      )}
    </svg>
  );
}

function Cross({
  at,
  box,
  className,
}: {
  at: { x: number; y: number };
  box: Box;
  className: string;
}) {
  if (!insideBox(at, box)) {
    return null;
  }
  return (
    <path
      d={`M${at.x - CROSS} ${at.y}H${at.x + CROSS}M${at.x} ${at.y - CROSS}V${at.y + CROSS}`}
      className={className}
    />
  );
}
