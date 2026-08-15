import { useRef, useState } from "react";
import { formatHz } from "../../components/format";
import type { PointReadout } from "./analysis";
import { type ChartView, seriesValues } from "./traces";

const WIDTH = 1200;
const HEIGHT = 300;
const LEFT = 64;
const RIGHT = 18;
const TOP = 16;
const BOTTOM = 38;
const PLOT_WIDTH = WIDTH - LEFT - RIGHT;
const PLOT_HEIGHT = HEIGHT - TOP - BOTTOM;
const GRID_FRACTIONS = [0, 0.25, 0.5, 0.75, 1];
/** Below this a drag was a click at a shaky hand, not a range selection. */
const ZOOM_MIN_FRACTION = 0.01;

interface Drag {
  from: number;
  to: number;
  zooming: boolean;
}

export function SweepChart({
  rows,
  view,
  marker,
  onMarker,
  onZoom,
}: {
  rows: readonly PointReadout[];
  view: ChartView;
  marker: number;
  onMarker: (index: number) => void;
  onZoom: (from: number, to: number) => void;
}) {
  const [drag, setDrag] = useState<Drag | null>(null);
  const surface = useRef<SVGSVGElement>(null);
  const domain = view.domain(seriesValues(view, rows));
  const span = domain.high - domain.low || 1;
  const lastIndex = Math.max(1, rows.length - 1);

  const x = (index: number) => LEFT + (index / lastIndex) * PLOT_WIDTH;
  const y = (value: number) =>
    TOP + ((domain.high - clamp(value, domain.low, domain.high)) / span) * PLOT_HEIGHT;

  function fractionAt(event: React.PointerEvent<SVGSVGElement>): number {
    const bounds = event.currentTarget.getBoundingClientRect();
    const localX = ((event.clientX - bounds.left) / bounds.width) * WIDTH;
    return clamp((localX - LEFT) / PLOT_WIDTH, 0, 1);
  }

  function begin(event: React.PointerEvent<SVGSVGElement>) {
    const fraction = fractionAt(event);
    event.currentTarget.setPointerCapture(event.pointerId);
    setDrag({ from: fraction, to: fraction, zooming: event.shiftKey });
    if (!event.shiftKey) {
      onMarker(Math.round(fraction * lastIndex));
    }
  }

  function extend(event: React.PointerEvent<SVGSVGElement>) {
    if (drag === null) {
      return;
    }
    const fraction = fractionAt(event);
    setDrag({ ...drag, to: fraction });
    if (!drag.zooming) {
      onMarker(Math.round(fraction * lastIndex));
    }
  }

  function finish() {
    if (drag !== null && drag.zooming && Math.abs(drag.to - drag.from) >= ZOOM_MIN_FRACTION) {
      const low = Math.round(Math.min(drag.from, drag.to) * lastIndex);
      const high = Math.round(Math.max(drag.from, drag.to) * lastIndex);
      onZoom(low, high);
    }
    setDrag(null);
  }

  function step(event: React.KeyboardEvent<SVGSVGElement>) {
    const delta = event.key === "ArrowLeft" ? -1 : event.key === "ArrowRight" ? 1 : 0;
    if (delta !== 0) {
      event.preventDefault();
      onMarker(clamp(marker + delta * (event.shiftKey ? 10 : 1), 0, rows.length - 1));
    }
  }

  const markerRow = rows[marker];
  return (
    <svg
      ref={surface}
      role="img"
      tabIndex={0}
      data-hotkeys="off"
      aria-label={`${view.label} against frequency`}
      viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
      className="w-full touch-none rounded-[3px] border border-line bg-plot-bg outline-none focus-visible:border-accent"
      onPointerDown={begin}
      onPointerMove={extend}
      onPointerUp={finish}
      onPointerCancel={finish}
      onKeyDown={step}
    >
      <title>{`${view.label} against frequency`}</title>
      {GRID_FRACTIONS.map((fraction) => (
        <g key={`h${fraction}`}>
          <line
            x1={LEFT}
            y1={TOP + fraction * PLOT_HEIGHT}
            x2={WIDTH - RIGHT}
            y2={TOP + fraction * PLOT_HEIGHT}
            stroke="var(--color-plot-grid)"
          />
          <text
            x={LEFT - 8}
            y={TOP + fraction * PLOT_HEIGHT + 4}
            textAnchor="end"
            className="fill-plot-ink-dim text-[10px] tabular-nums"
          >
            {view.format(domain.high - fraction * span)}
          </text>
        </g>
      ))}
      {GRID_FRACTIONS.map((fraction) => (
        <line
          key={`v${fraction}`}
          x1={LEFT + fraction * PLOT_WIDTH}
          y1={TOP}
          x2={LEFT + fraction * PLOT_WIDTH}
          y2={TOP + PLOT_HEIGHT}
          stroke="var(--color-plot-grid)"
        />
      ))}
      {view.series.map((series) => (
        <path
          key={series.key}
          d={pathFor(rows, series.valueOf, x, y)}
          fill="none"
          stroke={series.stroke}
          strokeWidth={1.75}
          vectorEffect="non-scaling-stroke"
        />
      ))}
      {view.series.length > 1 &&
        view.series.map((series, index) => (
          <text
            key={`legend-${series.key}`}
            x={WIDTH - RIGHT - 8 - (view.series.length - 1 - index) * 64}
            y={TOP + 14}
            textAnchor="end"
            fill={series.stroke}
            className="text-[10px]"
          >
            {series.label}
          </text>
        ))}
      {drag !== null && drag.zooming && (
        <rect
          x={LEFT + Math.min(drag.from, drag.to) * PLOT_WIDTH}
          y={TOP}
          width={Math.abs(drag.to - drag.from) * PLOT_WIDTH}
          height={PLOT_HEIGHT}
          className="fill-plot-ink/15 stroke-plot-ink/50"
        />
      )}
      {markerRow !== undefined && (
        <g>
          <line
            x1={x(marker)}
            y1={TOP}
            x2={x(marker)}
            y2={TOP + PLOT_HEIGHT}
            stroke="var(--color-plot-hold)"
            strokeWidth={1}
            vectorEffect="non-scaling-stroke"
          />
          {view.series.map((series) => {
            const value = series.valueOf(markerRow);
            return Number.isFinite(value) ? (
              <circle
                key={series.key}
                cx={x(marker)}
                cy={y(value)}
                r={3.5}
                fill={series.stroke}
                stroke="var(--color-plot-bg)"
              />
            ) : null;
          })}
          <text
            x={x(marker) + (marker > rows.length / 2 ? -6 : 6)}
            y={TOP + 12}
            textAnchor={marker > rows.length / 2 ? "end" : "start"}
            className="fill-plot-hold text-[10px] tabular-nums"
          >
            {formatHz(markerRow.frequencyHz)}
          </text>
        </g>
      )}
      <text x={LEFT} y={HEIGHT - 12} className="fill-plot-ink-dim text-[10px] tabular-nums">
        {formatHz(rows[0]?.frequencyHz ?? 0)}
      </text>
      <text
        x={WIDTH - RIGHT}
        y={HEIGHT - 12}
        textAnchor="end"
        className="fill-plot-ink-dim text-[10px] tabular-nums"
      >
        {formatHz(rows[rows.length - 1]?.frequencyHz ?? 0)}
      </text>
      <text
        x={LEFT + PLOT_WIDTH / 2}
        y={HEIGHT - 12}
        textAnchor="middle"
        className="fill-plot-ink-dim text-[10px]"
      >
        {view.unit === "" ? view.label : `${view.label} (${view.unit})`}
      </text>
    </svg>
  );
}

/** One polyline per series, broken wherever the quantity is not defined — an open circuit has
 * no group delay, and joining across the gap would draw a line through values nothing measured. */
function pathFor(
  rows: readonly PointReadout[],
  valueOf: (row: PointReadout) => number,
  x: (index: number) => number,
  y: (value: number) => number,
): string {
  let started = false;
  return rows
    .map((row, index) => {
      const value = valueOf(row);
      if (!Number.isFinite(value)) {
        started = false;
        return "";
      }
      const command = started ? "L" : "M";
      started = true;
      return `${command}${x(index).toFixed(2)},${y(value).toFixed(2)}`;
    })
    .join(" ")
    .trim();
}

function clamp(value: number, low: number, high: number): number {
  return Math.min(high, Math.max(low, value));
}
