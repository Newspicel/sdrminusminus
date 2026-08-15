import { useMemo, useRef, useState } from "react";
import { Button } from "../../components/BaseControls";
import { BTN_QUIET, LABEL, type Options } from "../../components/controls";
import { Segmented } from "../../components/Segmented";
import type { AntennaPoint, AntennaReport, AntennaSegment } from "../../lib/types";
import { designLabel, formatLength, type LengthUnit } from "./antenna";
import {
  type Angles,
  type Bounds,
  boundsOf,
  type Fit,
  fitTo,
  ISOMETRIC,
  MAX_PITCH,
  type PlanView,
  type Point2,
  place,
  planView,
  project,
  ROLE_LABEL,
  ROLE_STYLE,
  rolesIn,
  scaleBar,
  structureBounds,
  type Viewport,
} from "./geometry";

const HEIGHT = 320;
/** Kept clear at the foot of the drawing for the dimension line and the ruler, so an
 * annotation never lands on top of an element. */
const ANNOTATION_BAND = 56;
const VIEWPORT: Viewport = { width: 640, height: HEIGHT - ANNOTATION_BAND, padding: 34 };
const DIMENSION_RULE = HEIGHT - 40;
const RULER_LINE = HEIGHT - 12;
const GRID_DIVISIONS = 4;
const GRID_DROP = 0.06;
const DEGREES_PER_PIXEL = 0.4;

type ViewMode = "plan" | "orbit";

const MODE_OPTIONS: Options<ViewMode> = [
  { value: "plan", label: "2D" },
  { value: "orbit", label: "3D" },
];

function keyOf(from: AntennaPoint, to: AntennaPoint, label: string): string {
  return `${label}:${from.x_m},${from.y_m},${from.z_m}-${to.x_m},${to.y_m},${to.z_m}`;
}

export function AntennaView({
  report,
  unit,
  highlight,
  onHighlight,
}: {
  report: AntennaReport;
  unit: LengthUnit;
  highlight: string | null;
  onHighlight: (label: string | null) => void;
}) {
  const [mode, setMode] = useState<ViewMode>("plan");
  const [orbit, setOrbit] = useState<Angles>(ISOMETRIC);
  const drag = useRef<Point2 | null>(null);

  const bounds = useMemo(() => boundsOf(report.geometry), [report.geometry]);
  const plan = useMemo(() => planView(bounds), [bounds]);
  const angles = mode === "plan" ? plan.angles : orbit;
  const grid = useMemo(() => (mode === "orbit" ? groundGrid(bounds) : []), [mode, bounds]);

  const drawn = useMemo(() => {
    const lines = [
      ...grid.map((line) => ({ from: project(line[0], angles), to: project(line[1], angles) })),
      ...report.geometry.segments.map((segment) => ({
        from: project(segment.from, angles),
        to: project(segment.to, angles),
      })),
    ];
    const fit = fitTo(
      lines.flatMap((line) => [line.from, line.to]),
      VIEWPORT,
    );
    return { fit, feed: place(project(report.geometry.feed, angles), fit) };
  }, [grid, report.geometry, angles]);

  const ruler = scaleBar(drawn.fit.scale, VIEWPORT.width / 4, unit);
  const view = mode === "plan" ? plan.label : "Orbit — drag to turn";

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span className={LABEL}>{view}</span>
        <div className="flex items-center gap-2">
          {mode === "orbit" && (
            <Button type="button" className={BTN_QUIET} onClick={() => setOrbit(ISOMETRIC)}>
              Reset angle
            </Button>
          )}
          <Segmented label="Drawing view" value={mode} options={MODE_OPTIONS} onChange={setMode} />
        </div>
      </div>

      <svg
        data-hotkeys="off"
        viewBox={`0 0 ${VIEWPORT.width} ${HEIGHT}`}
        className={`max-h-72 w-full rounded-[3px] border border-line bg-panel-2 ${
          mode === "orbit" ? "cursor-grab active:cursor-grabbing touch-none" : ""
        }`}
        role="img"
        aria-label={`${designLabel(report.design.type)}, ${mode === "plan" ? plan.label.toLowerCase() : "seen from an angle"}`}
        onPointerDown={(event) => {
          if (mode !== "orbit") {
            return;
          }
          event.currentTarget.setPointerCapture(event.pointerId);
          drag.current = { x: event.clientX, y: event.clientY };
        }}
        onPointerMove={(event) => {
          const from = drag.current;
          if (from === null) {
            return;
          }
          drag.current = { x: event.clientX, y: event.clientY };
          setOrbit((angle) => turn(angle, event.clientX - from.x, event.clientY - from.y));
        }}
        onPointerUp={() => {
          drag.current = null;
        }}
        onPointerCancel={() => {
          drag.current = null;
        }}
      >
        {grid.map((line) => {
          const from = place(project(line[0], angles), drawn.fit);
          const to = place(project(line[1], angles), drawn.fit);
          return (
            <line
              key={keyOf(line[0], line[1], "grid")}
              x1={from.x}
              y1={from.y}
              x2={to.x}
              y2={to.y}
              className="stroke-line"
              strokeWidth={1}
            />
          );
        })}

        {report.geometry.segments.map((segment) => (
          <Piece
            key={keyOf(segment.from, segment.to, segment.label)}
            segment={segment}
            angles={angles}
            fit={drawn.fit}
            unit={unit}
            dimmed={highlight !== null && highlight !== segment.label}
            lit={highlight === segment.label}
            onHighlight={onHighlight}
          />
        ))}

        <circle
          cx={drawn.feed.x}
          cy={drawn.feed.y}
          r={4.5}
          className="fill-bg stroke-accent"
          strokeWidth={2}
        >
          <title>Feedpoint</title>
        </circle>

        {mode === "plan" && (
          <Dimensions
            geometry={report.geometry}
            plan={plan}
            fit={drawn.fit}
            angles={angles}
            unit={unit}
          />
        )}

        <g className="fill-ink-faint stroke-ink-faint">
          <line x1={16} y1={RULER_LINE} x2={16 + ruler.pixels} y2={RULER_LINE} strokeWidth={1.5} />
          <text x={16} y={RULER_LINE - 6} className="font-mono text-[10px]" stroke="none">
            {ruler.label}
          </text>
        </g>
      </svg>

      <div className="flex flex-wrap gap-x-4 gap-y-1">
        {rolesIn(report.geometry).map((role) => (
          <span key={role} className="flex items-center gap-1.5 font-mono text-[10px] text-ink-dim">
            <svg viewBox="0 0 16 4" className="h-1 w-4" aria-hidden>
              <line
                x1={0}
                y1={2}
                x2={16}
                y2={2}
                className={ROLE_STYLE[role].stroke}
                strokeWidth={4}
                strokeDasharray={ROLE_STYLE[role].dash}
              />
            </svg>
            {ROLE_LABEL[role]}
          </span>
        ))}
      </div>
    </div>
  );
}

function Piece({
  segment,
  angles,
  fit,
  unit,
  dimmed,
  lit,
  onHighlight,
}: {
  segment: AntennaSegment;
  angles: Angles;
  fit: Fit;
  unit: LengthUnit;
  dimmed: boolean;
  lit: boolean;
  onHighlight: (label: string | null) => void;
}) {
  const from = place(project(segment.from, angles), fit);
  const to = place(project(segment.to, angles), fit);
  const style = ROLE_STYLE[segment.role];
  const length = Math.hypot(
    segment.to.x_m - segment.from.x_m,
    segment.to.y_m - segment.from.y_m,
    segment.to.z_m - segment.from.z_m,
  );
  return (
    <line
      x1={from.x}
      y1={from.y}
      x2={to.x}
      y2={to.y}
      className={style.stroke}
      strokeWidth={lit ? style.width + 2 : style.width}
      strokeDasharray={style.dash}
      strokeLinecap="round"
      opacity={dimmed ? 0.3 : 1}
      onPointerEnter={() => onHighlight(segment.label)}
      onPointerLeave={() => onHighlight(null)}
    >
      <title>{`${segment.label} — ${formatLength(length, unit)}`}</title>
    </line>
  );
}

/** How wide and how tall the antenna itself is, bracketed off the drawing. */
function Dimensions({
  geometry,
  plan,
  fit,
  angles,
  unit,
}: {
  geometry: AntennaReport["geometry"];
  plan: PlanView;
  fit: Fit;
  angles: Angles;
  unit: LengthUnit;
}) {
  const metres = structureBounds(geometry);
  const drawn = geometry.segments
    .filter((segment) => segment.role !== "feedline")
    .flatMap((segment) => [segment.from, segment.to])
    .map((point) => place(project(point, angles), fit));
  if (drawn.length === 0) {
    return null;
  }
  const left = Math.min(...drawn.map((point) => point.x));
  const right = Math.max(...drawn.map((point) => point.x));
  const top = Math.min(...drawn.map((point) => point.y));
  const bottom = Math.max(...drawn.map((point) => point.y));
  const across = metres[plan.horizontal].size;
  const down = metres[plan.vertical].size;
  return (
    <g className="fill-ink-dim stroke-ink-faint">
      {across > 0 && (
        <>
          <line x1={left} y1={DIMENSION_RULE} x2={right} y2={DIMENSION_RULE} strokeWidth={1} />
          <line
            x1={left}
            y1={DIMENSION_RULE - 4}
            x2={left}
            y2={DIMENSION_RULE + 4}
            strokeWidth={1}
          />
          <line
            x1={right}
            y1={DIMENSION_RULE - 4}
            x2={right}
            y2={DIMENSION_RULE + 4}
            strokeWidth={1}
          />
          <text
            x={(left + right) / 2}
            y={DIMENSION_RULE - 7}
            textAnchor="middle"
            stroke="none"
            className="font-mono text-[10px]"
          >
            {formatLength(across, unit)}
          </text>
        </>
      )}
      {down > 0 && (
        <>
          <line x1={20} y1={top} x2={20} y2={bottom} strokeWidth={1} />
          <line x1={16} y1={top} x2={24} y2={top} strokeWidth={1} />
          <line x1={16} y1={bottom} x2={24} y2={bottom} strokeWidth={1} />
          <text
            x={12}
            y={(top + bottom) / 2}
            textAnchor="middle"
            dominantBaseline="middle"
            transform={`rotate(-90 12 ${(top + bottom) / 2})`}
            stroke="none"
            className="font-mono text-[10px]"
          >
            {formatLength(down, unit)}
          </text>
        </>
      )}
    </g>
  );
}

/** A square of ground under the antenna, so the 3D view has something to sit on. */
function groundGrid(bounds: Bounds): [AntennaPoint, AntennaPoint][] {
  const side = Math.max(bounds.x.size, bounds.z.size) * 1.2;
  if (side <= 0) {
    return [];
  }
  const centreX = (bounds.x.min + bounds.x.max) / 2;
  const centreZ = (bounds.z.min + bounds.z.max) / 2;
  const y = bounds.y.min - side * GRID_DROP;
  const step = side / GRID_DIVISIONS;
  const lines: [AntennaPoint, AntennaPoint][] = [];
  for (let index = 0; index <= GRID_DIVISIONS; index += 1) {
    const offset = -side / 2 + index * step;
    lines.push([
      { x_m: centreX + offset, y_m: y, z_m: centreZ - side / 2 },
      { x_m: centreX + offset, y_m: y, z_m: centreZ + side / 2 },
    ]);
    lines.push([
      { x_m: centreX - side / 2, y_m: y, z_m: centreZ + offset },
      { x_m: centreX + side / 2, y_m: y, z_m: centreZ + offset },
    ]);
  }
  return lines;
}

function turn(angles: Angles, dx: number, dy: number): Angles {
  return {
    yaw: angles.yaw + dx * DEGREES_PER_PIXEL,
    pitch: Math.min(MAX_PITCH, Math.max(-MAX_PITCH, angles.pitch + dy * DEGREES_PER_PIXEL)),
  };
}
