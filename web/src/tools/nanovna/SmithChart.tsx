import { formatHz } from "../../components/format";
import type { PointReadout } from "./analysis";

const SIZE = 340;
const CENTER = SIZE / 2;
const RADIUS = SIZE / 2 - 12;

/** Normalised resistances and reactances the grid is drawn at — the set a printed Smith chart
 * carries, which is what makes one readable at a glance. */
const RESISTANCES = [0.2, 0.5, 1, 2, 5];
const REACTANCES = [0.2, 0.5, 1, 2, 5];

export function SmithChart({
  rows,
  marker,
  onMarker,
}: {
  rows: readonly PointReadout[];
  marker: number;
  onMarker: (index: number) => void;
}) {
  const markerRow = rows[marker];

  function pick(event: React.PointerEvent<SVGSVGElement>) {
    const bounds = event.currentTarget.getBoundingClientRect();
    const u = (((event.clientX - bounds.left) / bounds.width) * SIZE - CENTER) / RADIUS;
    const v = -(((event.clientY - bounds.top) / bounds.height) * SIZE - CENTER) / RADIUS;
    let best = marker;
    let bestDistance = Number.POSITIVE_INFINITY;
    rows.forEach((row, index) => {
      const distance = Math.hypot(row.s11.re - u, row.s11.im - v);
      if (distance < bestDistance) {
        best = index;
        bestDistance = distance;
      }
    });
    onMarker(best);
  }

  return (
    <svg
      role="img"
      aria-label="S11 on a Smith chart"
      viewBox={`0 0 ${SIZE} ${SIZE}`}
      className="h-auto w-full max-w-[460px] touch-none rounded-[3px] border border-line bg-plot-bg"
      onPointerDown={pick}
      onPointerMove={(event) => {
        if (event.buttons === 1) {
          pick(event);
        }
      }}
    >
      <title>S11 on a Smith chart</title>
      <defs>
        <clipPath id="smith-unit">
          <circle cx={CENTER} cy={CENTER} r={RADIUS} />
        </clipPath>
      </defs>
      <circle
        cx={CENTER}
        cy={CENTER}
        r={RADIUS}
        fill="none"
        stroke="var(--color-plot-ink-dim)"
        strokeWidth={1}
      />
      <line
        x1={CENTER - RADIUS}
        y1={CENTER}
        x2={CENTER + RADIUS}
        y2={CENTER}
        stroke="var(--color-plot-grid)"
      />
      <g clipPath="url(#smith-unit)">
        {RESISTANCES.map((r) => (
          <circle
            key={`r${r}`}
            cx={CENTER + (r / (1 + r)) * RADIUS}
            cy={CENTER}
            r={(1 / (1 + r)) * RADIUS}
            fill="none"
            stroke="var(--color-plot-grid)"
          />
        ))}
        {REACTANCES.flatMap((x) =>
          [x, -x].map((value) => (
            <circle
              key={`x${value}`}
              cx={CENTER + RADIUS}
              cy={CENTER - (1 / value) * RADIUS}
              r={Math.abs(1 / value) * RADIUS}
              fill="none"
              stroke="var(--color-plot-grid)"
            />
          )),
        )}
      </g>
      <path
        d={rows
          .map(
            (row, index) =>
              `${index === 0 ? "M" : "L"}${(CENTER + row.s11.re * RADIUS).toFixed(2)},${(
                CENTER - row.s11.im * RADIUS
              ).toFixed(2)}`,
          )
          .join(" ")}
        fill="none"
        stroke="var(--color-plot-trace)"
        strokeWidth={1.75}
        vectorEffect="non-scaling-stroke"
      />
      {markerRow !== undefined && (
        <g>
          <circle
            cx={CENTER + markerRow.s11.re * RADIUS}
            cy={CENTER - markerRow.s11.im * RADIUS}
            r={4}
            fill="var(--color-plot-hold)"
            stroke="var(--color-plot-bg)"
          />
          <text x={8} y={SIZE - 8} className="fill-plot-hold text-[10px] tabular-nums">
            {formatHz(markerRow.frequencyHz)}
          </text>
        </g>
      )}
      <text x={CENTER - RADIUS + 2} y={CENTER - 6} className="fill-plot-ink-dim text-[9px]">
        0
      </text>
      <text
        x={CENTER + RADIUS - 2}
        y={CENTER - 6}
        textAnchor="end"
        className="fill-plot-ink-dim text-[9px]"
      >
        ∞
      </text>
    </svg>
  );
}
