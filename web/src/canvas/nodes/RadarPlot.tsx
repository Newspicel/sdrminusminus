import { useState } from "react";
import { GridCanvas } from "../../components/GridCanvas";
import { ColourScale, PlotAxis } from "../../components/PlotAxis";
import { linearTicks, type PlotRect, plotRect } from "../../components/plotFrame";
import { type Colormap, gridCellAt } from "../../gl/surface";
import { recordEvent } from "../../lib/diagnostics";
import type { RangeDopplerFrame } from "../../lib/frame";
import type { RadarTrack, RadarUpdate } from "../../lib/types";
import { useBoxSize } from "../../lib/useBoxSize";
import { useSurface } from "../../lib/useSurface";
import {
  dopplerSpanHz,
  hoverText,
  LIGHT_SPEED_M_S,
  levelDb,
  type PlotAxes,
  plotAxes,
  pxToSurface,
  rangeSpanKm,
  SURFACE_DB_MAX,
  SURFACE_DB_MIN,
  type SurfacePoint,
  surfaceToPx,
  velocityMps,
} from "./radar";

const GUTTERS = { left: 36, right: 40, top: 16, bottom: 18 };
const TICKS = 5;
const DETECTION_R = 4;
const TRACK_HALF = 4;
const TRUTH_R = 5;

export function RadarPlot({
  node,
  update,
  dim,
  colormap,
  selected,
}: {
  node: string;
  update: RadarUpdate | null;
  dim: boolean;
  colormap: Colormap;
  selected: number | null;
}) {
  const [frame, setFrame] = useState<RangeDopplerFrame | null>(null);
  const [hover, setHover] = useState<{ x: number; y: number } | null>(null);
  const [ref, size] = useBoxSize<HTMLDivElement>();
  useSurface(node, (surface) => {
    if (surface.kind === "range_doppler") {
      setFrame(surface.frame);
    } else {
      recordEvent("warn", "radar", `unexpected ${surface.kind} surface`);
    }
  });
  const axes = plotAxes(frame, update);
  const plot = plotRect(size.width, size.height, GUTTERS);
  const grid =
    frame === null ? null : { cols: frame.ranges, rows: frame.dopplers, cells: frame.cells };
  return (
    <div ref={ref} className="relative min-h-48 flex-1 bg-plot-bg">
      <div className={`absolute ${dim ? "opacity-40" : ""}`} style={GUTTERS}>
        <GridCanvas frame={grid} colormap={colormap} flipY />
      </div>
      <span className="absolute top-0.5 left-1 font-mono text-[9px] text-ink-faint">Hz</span>
      <span className="absolute top-0.5 right-1 font-mono text-[9px] text-ink-faint">m/s</span>
      <span className="absolute right-1 bottom-0.5 font-mono text-[9px] text-ink-faint">km</span>
      <span className="absolute top-0.5 left-1/2 -translate-x-1/2">
        <ColourScale
          colormap={colormap}
          min={axes?.dbMin ?? SURFACE_DB_MIN}
          max={axes?.dbMax ?? SURFACE_DB_MAX}
          unit="dB"
        />
      </span>
      {axes !== null && plot.w > 0 && plot.h > 0 && (
        <svg
          aria-hidden
          className="pointer-events-none absolute inset-0 size-full overflow-visible"
        >
          <RadarAxes axes={axes} plot={plot} />
          <svg x={plot.x} y={plot.y} width={plot.w} height={plot.h}>
            <RadarMarks update={update} axes={axes} plot={plot} selected={selected} />
          </svg>
        </svg>
      )}
      {axes !== null && (
        <div
          className="absolute"
          style={GUTTERS}
          onPointerMove={(event) => {
            const box = event.currentTarget.getBoundingClientRect();
            setHover({ x: event.clientX - box.left, y: event.clientY - box.top });
          }}
          onPointerLeave={() => setHover(null)}
        >
          {hover !== null && (
            <span className="pointer-events-none absolute top-1 left-1 rounded-[2px] bg-bg/80 px-1 font-mono text-[10px] tabular-nums text-ink">
              {hoverLabel(hover, axes, plot, frame)}
            </span>
          )}
        </div>
      )}
    </div>
  );
}

function hoverLabel(
  hover: { x: number; y: number },
  axes: PlotAxes,
  plot: PlotRect,
  frame: RangeDopplerFrame | null,
): string {
  const box = { w: plot.w, h: plot.h };
  const point = pxToSurface(hover.x, hover.y, axes, box);
  const cell =
    frame === null
      ? null
      : gridCellAt(hover.x, hover.y, box, { cols: frame.ranges, rows: frame.dopplers }, true);
  const level =
    cell === null || frame === null ? undefined : frame.cells[cell.row * frame.ranges + cell.col];
  return hoverText(point, axes.carrierHz, level === undefined ? null : levelDb(level, axes));
}

function RadarAxes({ axes, plot }: { axes: PlotAxes; plot: PlotRect }) {
  const box = { w: plot.w, h: plot.h };
  const range = rangeSpanKm(axes);
  const doppler = dopplerSpanHz(axes);
  const rangeTicks = linearTicks(
    range.lo,
    range.hi,
    TICKS,
    (km) => surfaceToPx({ rangeKm: km, dopplerHz: 0 }, axes, box).x,
  );
  const dopplerTicks = linearTicks(
    doppler.lo,
    doppler.hi,
    TICKS,
    (hz) => surfaceToPx({ rangeKm: 0, dopplerHz: hz }, axes, box).y,
  );
  const fast = velocityMps(doppler.lo, axes.carrierHz);
  const slow = velocityMps(doppler.hi, axes.carrierHz);
  const speedTicks =
    fast === null || slow === null
      ? []
      : linearTicks(
          slow,
          fast,
          TICKS,
          (mps) =>
            surfaceToPx(
              { rangeKm: 0, dopplerHz: (-mps * axes.carrierHz) / LIGHT_SPEED_M_S },
              axes,
              box,
            ).y,
        );
  return (
    <>
      <rect
        x={plot.x}
        y={plot.y}
        width={plot.w}
        height={plot.h}
        className="fill-none stroke-line"
      />
      <PlotAxis side="bottom" ticks={rangeTicks} plot={plot} />
      <PlotAxis side="left" ticks={dopplerTicks} plot={plot} />
      <PlotAxis side="right" ticks={speedTicks} plot={plot} />
    </>
  );
}

function RadarMarks({
  update,
  axes,
  plot,
  selected,
}: {
  update: RadarUpdate | null;
  axes: PlotAxes;
  plot: PlotRect;
  selected: number | null;
}) {
  if (update === null) {
    return null;
  }
  const box = { w: plot.w, h: plot.h };
  const at = (point: SurfacePoint) => surfaceToPx(point, axes, box);
  return (
    <>
      {update.detections.map((detection, index) => {
        const { x, y } = at({ rangeKm: detection.range_km, dopplerHz: detection.doppler_hz });
        return (
          <circle
            key={index}
            cx={x}
            cy={y}
            r={DETECTION_R}
            className="fill-none stroke-plot-ink/60"
          />
        );
      })}
      {update.tracks.map((track) => (
        <TrackMark key={track.id} track={track} at={at} selected={track.id === selected} />
      ))}
      {update.truth
        .filter((truth) => truth.in_view)
        .map((truth) => {
          const { x, y } = at({ rangeKm: truth.range_km, dopplerHz: truth.doppler_hz });
          return (
            <path
              key={truth.icao}
              d={`M${x} ${y - TRUTH_R}L${x + TRUTH_R} ${y + TRUTH_R}L${x - TRUTH_R} ${y + TRUTH_R}Z`}
              className="fill-none stroke-port-events"
            >
              <title>{truth.callsign ?? truth.icao}</title>
            </path>
          );
        })}
    </>
  );
}

function TrackMark({
  track,
  at,
  selected,
}: {
  track: RadarTrack;
  at: (point: SurfacePoint) => { x: number; y: number };
  selected: boolean;
}) {
  const head = at({ rangeKm: track.range_km, dopplerHz: track.doppler_hz });
  const trail = track.trail
    .map((point) => at({ rangeKm: point.range_km, dopplerHz: point.doppler_hz }))
    .map(({ x, y }) => `${x.toFixed(1)},${y.toFixed(1)}`)
    .join(" ");
  const coasting = track.state === "coasting";
  const width = selected ? 2.5 : 1.25;
  return (
    <g className="stroke-accent">
      {track.trail.length > 1 && (
        <polyline points={trail} className="fill-none opacity-60" strokeWidth={1} />
      )}
      <rect
        x={head.x - TRACK_HALF}
        y={head.y - TRACK_HALF}
        width={TRACK_HALF * 2}
        height={TRACK_HALF * 2}
        className="fill-none"
        strokeWidth={width}
        strokeDasharray={coasting ? "2 2" : undefined}
      />
      <text
        x={head.x + TRACK_HALF + 2}
        y={head.y - TRACK_HALF}
        className="fill-accent stroke-none font-mono text-[9px]"
      >
        T{track.id}
      </text>
    </g>
  );
}
