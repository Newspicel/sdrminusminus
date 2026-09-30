import { useEffect, useRef, useState } from "react";
import { GridCanvas } from "../../components/GridCanvas";
import { PlotAxis } from "../../components/PlotAxis";
import { linearTicks, type PlotRect, plotRect } from "../../components/plotFrame";
import { attachTrail, type TrailView } from "../../gl/spatial";
import type { Colormap } from "../../gl/surface";
import { recordEvent } from "../../lib/diagnostics";
import type { SpatialSpectrumFrame } from "../../lib/frame";
import type { SpatialPeak } from "../../lib/types";
import { useBoxSize } from "../../lib/useBoxSize";
import { useSurface } from "../../lib/useSurface";
import { SurfaceRefused } from "./SurfaceRefused";
import {
  BEARING_TICKS,
  type BearingFrame,
  bearingHue,
  belowPeakDb,
  cursorText,
  hoverAt,
  peakBearing,
  rotateRows,
  type SpatialView,
} from "./spatialSpectrum";

const GUTTERS = { left: 32, right: 6, top: 4, bottom: 18 };
const CROSS = 4;
const FREQ_TICKS = 5;

export function SpatialPlot({
  node,
  known,
  view,
  bearingFrame,
  offsetDeg,
  colormap,
  peaks,
  dim,
}: {
  node: string;
  known: boolean;
  view: SpatialView;
  bearingFrame: BearingFrame;
  offsetDeg: number;
  colormap: Colormap;
  peaks: readonly SpatialPeak[];
  dim: boolean;
}) {
  const [frame, setFrame] = useState<SpatialSpectrumFrame | null>(null);
  const [hover, setHover] = useState<{ x: number; y: number } | null>(null);
  const [ref, size] = useBoxSize<HTMLDivElement>();
  const refused = useSurface(known ? node : null, (surface) => {
    if (surface.kind === "spatial_spectrum") {
      setFrame(surface.frame);
    } else {
      recordEvent("warn", "spatial", `unexpected ${surface.kind} surface`);
    }
  });
  const plot = plotRect(size.width, size.height, GUTTERS);
  const rotated = frame === null ? null : rotateRows(frame, offsetDeg);
  return (
    <div ref={ref} className="relative min-h-40 flex-1 bg-plot-bg">
      <div className={`absolute ${dim ? "opacity-40" : ""}`} style={GUTTERS}>
        {view === "map" ? (
          <GridCanvas
            frame={
              frame === null || rotated === null
                ? null
                : { cols: frame.bins, rows: frame.bearings, cells: rotated }
            }
            colormap={colormap}
            flipY={false}
          />
        ) : (
          <TrailCanvas frame={frame} offsetDeg={offsetDeg} />
        )}
      </div>
      <span className="absolute right-1 bottom-0.5 font-mono text-[9px] text-ink-faint">MHz</span>
      <span className="absolute top-0.5 left-1 font-mono text-[9px] text-ink-faint">
        {view === "map" ? "°" : "t"}
      </span>
      <SurfaceRefused text={refused} />
      {frame !== null && plot.w > 0 && plot.h > 0 && (
        <svg
          aria-hidden
          className="pointer-events-none absolute inset-0 size-full overflow-visible"
        >
          <SpatialAxes frame={frame} plot={plot} view={view} />
          {view === "map" && (
            <svg x={plot.x} y={plot.y} width={plot.w} height={plot.h}>
              <PeakMarks
                frame={frame}
                plot={plot}
                peaks={peaks}
                bearingFrame={bearingFrame}
                offsetDeg={offsetDeg}
              />
            </svg>
          )}
        </svg>
      )}
      {frame !== null && view === "map" && (
        <div
          className="absolute"
          style={GUTTERS}
          onPointerMove={(event) => {
            const box = event.currentTarget.getBoundingClientRect();
            setHover({ x: event.clientX - box.left, y: event.clientY - box.top });
          }}
          onPointerLeave={() => setHover(null)}
        >
          {hover !== null && rotated !== null && (
            <span className="pointer-events-none absolute top-1 left-1 rounded-[2px] bg-bg/80 px-1 font-mono text-[10px] tabular-nums text-ink">
              {hoverLabel(hover, plot, frame, rotated)}
            </span>
          )}
        </div>
      )}
    </div>
  );
}

function hoverLabel(
  hover: { x: number; y: number },
  plot: PlotRect,
  frame: SpatialSpectrumFrame,
  rotated: Uint8Array,
): string {
  const at = hoverAt(hover.x, hover.y, { w: plot.w, h: plot.h }, frame);
  const level = rotated[at.row * frame.bins + at.col];
  return cursorText(at.hz, at.deg, level === undefined ? null : belowPeakDb(level, frame));
}

function SpatialAxes({
  frame,
  plot,
  view,
}: {
  frame: SpatialSpectrumFrame;
  plot: PlotRect;
  view: SpatialView;
}) {
  const lo = frame.centerHz - frame.spanHz / 2;
  const hi = frame.centerHz + frame.spanHz / 2;
  const freq = linearTicks(lo / 1e6, hi / 1e6, FREQ_TICKS, (mhz) =>
    hi > lo ? ((mhz * 1e6 - lo) / (hi - lo)) * plot.w : 0,
  );
  const bearings = BEARING_TICKS.map((deg) => ({ px: (deg / 360) * plot.h, label: String(deg) }));
  return (
    <>
      <rect
        x={plot.x}
        y={plot.y}
        width={plot.w}
        height={plot.h}
        className="fill-none stroke-line"
      />
      <PlotAxis side="bottom" ticks={freq} plot={plot} />
      {view === "map" && <PlotAxis side="left" ticks={bearings} plot={plot} />}
    </>
  );
}

function PeakMarks({
  frame,
  plot,
  peaks,
  bearingFrame,
  offsetDeg,
}: {
  frame: SpatialSpectrumFrame;
  plot: PlotRect;
  peaks: readonly SpatialPeak[];
  bearingFrame: BearingFrame;
  offsetDeg: number;
}) {
  const lo = frame.centerHz - frame.spanHz / 2;
  return (
    <>
      {peaks.map((peak) => {
        const x = frame.spanHz > 0 ? ((peak.freq_hz - lo) / frame.spanHz) * plot.w : 0;
        const y = (bearingHue(peakBearing(peak, bearingFrame, offsetDeg)) / 360) * plot.h;
        return (
          <path
            key={`${peak.freq_hz}:${peak.bearing_deg}`}
            d={`M${x - CROSS} ${y - CROSS}L${x + CROSS} ${y + CROSS}M${x - CROSS} ${y + CROSS}L${x + CROSS} ${y - CROSS}`}
            className="stroke-plot-ink"
            strokeWidth={1.5}
          />
        );
      })}
    </>
  );
}

function TrailCanvas({
  frame,
  offsetDeg,
}: {
  frame: SpatialSpectrumFrame | null;
  offsetDeg: number;
}) {
  const canvas = useRef<HTMLCanvasElement | null>(null);
  const view = useRef<TrailView | null>(null);
  const pushed = useRef<SpatialSpectrumFrame | null>(null);
  useEffect(() => {
    const element = canvas.current;
    if (element === null) {
      return;
    }
    const attached = attachTrail(element);
    view.current = attached;
    return () => {
      attached.dispose();
      view.current = null;
      pushed.current = null;
    };
  }, []);
  useEffect(() => {
    if (frame === null || frame === pushed.current) {
      return;
    }
    pushed.current = frame;
    view.current?.push(frame, offsetDeg);
  }, [frame, offsetDeg]);
  return <canvas ref={canvas} aria-hidden className="absolute inset-0 size-full" />;
}
