import { useState } from "react";
import { PlotAxis } from "../../components/PlotAxis";
import { linearTicks, type PlotRect, plotRect } from "../../components/plotFrame";
import { recordEvent } from "../../lib/diagnostics";
import type { VisibilityFrame } from "../../lib/frame";
import { useBoxSize } from "../../lib/useBoxSize";
import { useSurface } from "../../lib/useSurface";
import {
  baselineSeries,
  FRINGE_POINTS,
  fringePath,
  linePath,
  PHASE_SPAN,
  phasePath,
} from "./correlator";

const GUTTERS = { left: 34, right: 6, top: 12, bottom: 18 };
const GAP = 16;
const FREQ_TICKS = 5;
const LEVEL_TICKS = 3;
const PHASE_TICKS = [-180, -90, 0, 90, 180] as const;
const FRINGE_W = 72;
const FRINGE_H = 22;

export function CorrelatorPlots({
  node,
  index,
  dim,
}: {
  node: string;
  index: number;
  dim: boolean;
}) {
  const [frame, setFrame] = useState<VisibilityFrame | null>(null);
  const [ref, size] = useBoxSize<HTMLDivElement>();
  useSurface(node, (surface) => {
    if (surface.kind === "visibility") {
      setFrame(surface.frame);
    } else {
      recordEvent("warn", "correlator", `unexpected ${surface.kind} surface`);
    }
  });
  const whole = plotRect(size.width, size.height, GUTTERS);
  const half = Math.max(0, (whole.h - GAP) / 2);
  const amplitude = { x: whole.x, y: whole.y, w: whole.w, h: half };
  const phase = { x: whole.x, y: whole.y + half + GAP, w: whole.w, h: half };
  return (
    <div ref={ref} className="relative min-h-40 flex-1 bg-plot-bg">
      <span className="absolute top-0 left-1 font-mono text-[9px] text-ink-faint">
        Amplitude dB
      </span>
      <span
        className="absolute left-1 font-mono text-[9px] text-ink-faint"
        style={{ top: phase.y - GUTTERS.top }}
      >
        Phase °
      </span>
      <span className="absolute right-1 bottom-0.5 font-mono text-[9px] text-ink-faint">MHz</span>
      {frame !== null && whole.w > 0 && half > 0 && (
        <svg
          aria-hidden
          className={`pointer-events-none absolute inset-0 size-full overflow-visible ${dim ? "opacity-50" : ""}`}
        >
          <Plots frame={frame} index={index} amplitude={amplitude} phase={phase} />
        </svg>
      )}
    </div>
  );
}

function Plots({
  frame,
  index,
  amplitude,
  phase,
}: {
  frame: VisibilityFrame;
  index: number;
  amplitude: PlotRect;
  phase: PlotRect;
}) {
  const series = baselineSeries(frame, index);
  const x = {
    lo: frame.centerHz - frame.spanHz / 2,
    hi: frame.centerHz + frame.spanHz / 2,
  };
  const y = { lo: frame.dbMin, hi: frame.dbMax };
  const freq = linearTicks(x.lo / 1e6, x.hi / 1e6, FREQ_TICKS, (mhz) =>
    x.hi > x.lo ? ((mhz * 1e6 - x.lo) / (x.hi - x.lo)) * phase.w : 0,
  );
  const levels = linearTicks(y.lo, y.hi, LEVEL_TICKS, (db) =>
    y.hi > y.lo ? (1 - (db - y.lo) / (y.hi - y.lo)) * amplitude.h : 0,
  );
  const phases = PHASE_TICKS.map((deg) => ({
    px: (1 - (deg - PHASE_SPAN.lo) / (PHASE_SPAN.hi - PHASE_SPAN.lo)) * phase.h,
    label: String(deg),
  }));
  return (
    <>
      <rect {...box(amplitude)} className="fill-none stroke-line" />
      <rect {...box(phase)} className="fill-none stroke-line" />
      <PlotAxis side="left" ticks={levels} plot={amplitude} />
      <PlotAxis side="left" ticks={phases} plot={phase} />
      <PlotAxis side="bottom" ticks={freq} plot={phase} />
      <path
        transform={`translate(${amplitude.x} ${amplitude.y})`}
        d={linePath(series.hz, series.db, { w: amplitude.w, h: amplitude.h }, x, y)}
        className="fill-none stroke-plot-trace"
        strokeWidth={1.25}
      />
      <path
        transform={`translate(${phase.x} ${phase.y})`}
        d={phasePath(series.hz, series.deg, { w: phase.w, h: phase.h }, x)}
        className="fill-none stroke-accent"
        strokeWidth={1.25}
      />
    </>
  );
}

function box(rect: PlotRect): { x: number; y: number; width: number; height: number } {
  return { x: rect.x, y: rect.y, width: rect.w, height: rect.h };
}

export function FringeLine({ history }: { history: readonly number[] }) {
  return (
    <svg
      role="img"
      aria-label="Fringe phase"
      width={FRINGE_W}
      height={FRINGE_H}
      className="shrink-0 rounded-[2px] bg-well"
    >
      <title>{`Phase over the last ${FRINGE_POINTS} results`}</title>
      <path
        d={fringePath(history, { w: FRINGE_W, h: FRINGE_H })}
        className="fill-none stroke-accent"
        strokeWidth={1}
      />
    </svg>
  );
}
